//! Reading discs: a folder holding VIDEO_TS or BDMV, or a disc image read without mounting it.
//! Every file on the volume is listed, not just the video folders, so added junk files show up.

pub mod bluray;
pub mod dvd;
pub mod image;
pub mod listing;
pub mod scanner;

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

const IMAGE_EXTENSIONS: [&str; 3] = ["iso", "img", "udf"];
const IGNORED_FOLDERS: [&str; 2] = ["System Volume Information", "$RECYCLE.BIN"];

pub enum FileData {
    Folder(PathBuf),
    Image(image::Entry),
}

pub struct SourceFile {
    /// '/'-separated, relative to the disc root.
    pub path: String,
    pub size: u64,
    data: FileData,
}

pub struct DiscSource {
    /// The volume label (or the folder/image name when there is none).
    pub label: String,
    /// "dvd" or "blu-ray".
    pub format: &'static str,
    /// Every file, sorted by path.
    pub files: Vec<SourceFile>,
    image: Option<File>,
}

impl DiscSource {
    pub fn read(&self, file: &SourceFile) -> io::Result<Vec<u8>> {
        match &file.data {
            FileData::Folder(path) => fs::read(path),
            FileData::Image(entry) => {
                image::read_entry(self.image.as_ref().expect("image source"), entry)
            }
        }
    }

    /// The file at a disc path, compared case-insensitively.
    pub fn find(&self, path: &str) -> Option<&SourceFile> {
        self.files
            .iter()
            .find(|f| f.path.eq_ignore_ascii_case(path))
    }

    pub fn open(path: &Path) -> Result<DiscSource, String> {
        let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if is_image(&full) {
            return open_image(&full);
        }

        if !full.is_dir() {
            return Err(format!(
                "not a disc image (.iso/.img/.udf) or a folder: {}",
                path.display()
            ));
        }

        // VIDEO_TS or BDMV itself was given.
        let root = match full
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(format_of_folder)
        {
            Some(_) => full.parent().map(Path::to_path_buf).unwrap_or(full.clone()),
            None => full,
        };

        let mut files = Vec::new();
        list_folder(&root, &root, &mut files)
            .map_err(|e| format!("could not list {}: {e}", root.display()))?;
        create(
            files,
            folder_label(&root),
            None,
            &root.display().to_string(),
        )
    }
}

pub fn is_image(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub fn is_disc_folder(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.file_type().is_ok_and(|t| t.is_dir())
                && e.file_name().to_str().and_then(format_of_folder).is_some()
        })
    })
}

fn format_of_folder(name: &str) -> Option<&'static str> {
    if name.eq_ignore_ascii_case("VIDEO_TS") {
        Some("dvd")
    } else if name.eq_ignore_ascii_case("BDMV") {
        Some("blu-ray")
    } else {
        None
    }
}

fn open_image(path: &Path) -> Result<DiscSource, String> {
    let file = File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let volume =
        image::read_volume(&file).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let files = volume
        .files
        .into_iter()
        .filter(|e| !is_ignored(&e.path))
        .map(|e| SourceFile {
            path: e.path.clone(),
            size: e.size,
            data: FileData::Image(e),
        })
        .collect();
    let label = if volume.label.trim().is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        volume.label.trim().to_string()
    };
    create(files, label, Some(file), &path.display().to_string())
}

fn create(
    mut files: Vec<SourceFile>,
    label: String,
    image: Option<File>,
    place: &str,
) -> Result<DiscSource, String> {
    let mut formats: Vec<&'static str> = files
        .iter()
        .filter_map(|f| format_of_folder(f.path.split('/').next().unwrap_or("")))
        .collect();
    formats.sort_unstable();
    formats.dedup();
    if formats.len() != 1 {
        return Err(if formats.is_empty() {
            format!("no VIDEO_TS or BDMV files in {place}")
        } else {
            format!("{place} holds both VIDEO_TS and BDMV")
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(DiscSource {
        label,
        format: formats[0],
        files,
        image,
    })
}

fn list_folder(root: &Path, dir: &Path, out: &mut Vec<SourceFile>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if is_system(&meta) {
            continue;
        }

        let path = entry.path();
        if meta.is_dir() {
            list_folder(root, &path, out)?;
        } else if meta.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let relative = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if !is_ignored(&relative) {
                out.push(SourceFile {
                    path: relative,
                    size: meta.len(),
                    data: FileData::Folder(path),
                });
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
fn is_system(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x4 != 0
}

#[cfg(not(windows))]
fn is_system(_: &fs::Metadata) -> bool {
    false
}

fn is_ignored(relative: &str) -> bool {
    IGNORED_FOLDERS.iter().any(|f| {
        relative.len() > f.len()
            && relative.as_bytes()[f.len()] == b'/'
            && relative[..f.len()].eq_ignore_ascii_case(f)
    })
}

/// A drive's volume label when the folder is a drive root, otherwise the folder's name.
fn folder_label(folder: &Path) -> String {
    #[cfg(windows)]
    if folder.parent().is_none()
        && let Some(label) = windows_volume_label(folder)
    {
        return label;
    }

    match folder.file_name() {
        Some(name) if !name.is_empty() => name.to_string_lossy().into_owned(),
        _ => "disc".to_string(),
    }
}

#[cfg(windows)]
fn windows_volume_label(root: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            fs_name: *mut u16,
            fs_name_len: u32,
        ) -> i32;
    }

    let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    if wide.last() != Some(&(b'\\' as u16)) {
        wide.push(b'\\' as u16);
    }
    wide.push(0);
    let mut name = [0u16; 261];
    // SAFETY: valid NUL-terminated path and a buffer of the stated length; other outputs are optional.
    let ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            name.as_mut_ptr(),
            name.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    let len = name.iter().position(|&c| c == 0).unwrap_or(0);
    (ok != 0 && len > 0).then(|| String::from_utf16_lossy(&name[..len]))
}
