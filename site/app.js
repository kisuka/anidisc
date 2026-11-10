// AniDiscDB browser. Reads the static API built by `anidisc build`:
//   api/index.json, api/series/<slug>.json, api/hash/<HASH>.json
"use strict";

const view = document.getElementById("view");
const q = document.getElementById("q");
let index = null;

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const isHash = (s) => /^[0-9a-fA-F]{32}$/.test(s.trim());
const fold = (s) => String(s ?? "").normalize("NFKD").replace(/[̀-ͯ]/g, "").toLowerCase();

async function getJson(path) {
  const res = await fetch(path, { cache: "no-cache" });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`${path}: ${res.status}`);
  return res.json();
}

async function loadIndex() {
  if (!index) {
    index = await getJson("api/index.json");
    document.getElementById("stats").textContent = `${index.series.length} series · ${index.discCount} discs`;
    if (index.repositoryUrl) {
      document.getElementById("contribute").innerHTML =
        `Missing a disc? <a href="${esc(index.repositoryUrl)}#contributing">Contribute</a>.`;
    }
  }
  return index;
}

function seriesName(s) {
  return s.year ? `${s.title} (${s.year})` : s.title;
}

async function renderSearch(text) {
  const idx = await loadIndex();
  const needle = fold(text.trim());
  const hits = needle
    ? idx.series.filter((s) => [s.title, s.originalTitle, ...(s.aliases ?? [])].some((t) => fold(t).includes(needle)))
    : idx.series;
  if (hits.length === 0) {
    view.innerHTML = `<p class="muted">No series match “${esc(text)}”.</p>`;
    return;
  }
  view.innerHTML = `<ul class="list">${hits.map((s) => `
    <li><a href="#/s/${esc(s.slug)}">
      <span>${esc(seriesName(s))}${s.originalTitle ? ` <span class="sub">${esc(s.originalTitle)}</span>` : ""}</span>
      <span class="sub">${s.discs} disc${s.discs === 1 ? "" : "s"}</span>
    </a></li>`).join("")}</ul>`;
}

// AniDB numbering: 1 is a regular episode; S1 special, C1 credits, T1 trailer, P1 parody, O1 other.
const KINDS = { S: "Special", C: "Credits", T: "Trailer", P: "Parody", O: "Other" };
const epLabel = (n) => (typeof n === "number" ? `Episode ${n}` : `${KINDS[n[0]] ?? "Episode"} ${n}`);

function describe(t, series) {
  if (t.episodes?.length) {
    const main = series.anidb?.[0];
    const work = t.anidb ?? main;
    return t.episodes.map((n) => {
      const ep = series.episodes?.find((e) => e.number === n && (e.anidb ?? main) === work);
      return ep?.title ? `${esc(epLabel(n))}: ${esc(ep.title)}` : esc(epLabel(n));
    }).join("<br>");
  }
  if (!t.type) return `<span class="muted">not identified</span>`;
  return esc(t.name ? `${t.type}: ${t.name}` : t.type);
}

function discCard(disc, slug, series, highlight) {
  const d = disc;
  const titles = d.titles ?? [];
  const rows = titles.map((t) => `
    <tr class="${t.type ? "" : "unknown"}">
      <td class="mono">${esc(t.title ?? t.playlist)}</td>
      <td class="mono">${esc(t.duration)}</td>
      <td>${t.chapters ?? ""}</td>
      <td class="desc">${describe(t, series)}</td>
    </tr>`).join("");
  return `
    <section class="disc${highlight ? " hit" : ""}" id="d-${esc(d.contentHash)}">
      <header>
        <h3>${esc(d.label || "Disc")}</h3>
        ${d.catalogNumber ? `<span class="chip">${esc(d.catalogNumber)}</span>` : ""}
        ${d.pressing ? `<span class="chip">${esc(d.pressing)}</span>` : ""}
        ${d.volumeLabel ? `<span class="muted mono">${esc(d.volumeLabel)}</span>` : ""}
      </header>
      <div class="hash mono">${esc(d.contentHash)}</div>
      ${d.notes ? `<p>${esc(d.notes)}</p>` : ""}
      ${titles.length ? `<div class="table-wrap"><table>
        <thead><tr><th>${d.format === "dvd" ? "Title" : "Playlist"}</th><th>Length</th><th>Ch.</th><th>Contents</th></tr></thead>
        <tbody>${rows}</tbody></table></div>` : ""}
      ${d.files?.length ? `<details class="files"><summary>${d.files.length} files on the reference disc</summary>
        <div class="table-wrap"><table><tbody>${d.files.map((f) =>
          `<tr><td class="mono">${esc(f.path)}</td><td class="mono num">${f.size.toLocaleString("en-US")}</td></tr>`).join("")}
        </tbody></table></div></details>` : ""}
    </section>`;
}

async function renderSeries(slug, hitHash) {
  const rec = await getJson(`api/series/${encodeURIComponent(slug)}.json`);
  if (!rec) {
    view.innerHTML = `<p>Series not found.</p>`;
    return;
  }
  const s = rec.series;
  document.title = `${seriesName(s)} · AniDiscDB`;
  const anidb = (s.anidb ?? []).map((id) => `<a class="chip" href="https://anidb.net/anime/${id}" rel="noreferrer">AniDB ${id}</a>`).join("");
  const aliases = (s.aliases ?? []).map((a) => `<span class="chip">${esc(a)}</span>`).join("");
  const releases = rec.releases.map((r) => {
    const rel = r.release;
    const facts = [rel.media, rel.region, rel.publisher, rel.releaseDate, rel.edition, rel.upc && `UPC ${rel.upc}`].filter(Boolean);
    return `
      <h2>${esc(rel.name || `${rel.media} ${rel.region}`)}</h2>
      <div class="chips">${facts.map((f) => `<span class="chip">${esc(f)}</span>`).join("")}</div>
      ${rel.notes ? `<p>${esc(rel.notes)}</p>` : ""}
      ${(r.discs ?? []).map((d) => discCard(d.disc, d.slug, s, d.disc.contentHash === hitHash)).join("")}`;
  }).join("");
  const episodes = s.episodes?.length
    ? `<h2>Episodes</h2><ul class="episodes">${s.episodes.map((e) =>
        `<li><span class="mono epno">${esc(e.number)}</span><span>${esc(e.title ?? "")}${e.romajiTitle || e.originalTitle ? `<span class="alt muted">${[e.romajiTitle, e.originalTitle].filter(Boolean).map(esc).join(" · ")}</span>` : ""}${e.anidb && e.anidb !== s.anidb?.[0] ? ` <span class="muted">(AniDB ${e.anidb})</span>` : ""}</span></li>`).join("")}</ul>`
    : "";
  view.innerHTML = `
    <h1>${esc(seriesName(s))}</h1>
    ${s.originalTitle ? `<div class="muted">${esc(s.originalTitle)}</div>` : ""}
    <div class="chips">${anidb}${aliases}</div>
    ${s.notes ? `<p>${esc(s.notes)}</p>` : ""}
    ${releases}
    ${episodes}`;
  if (hitHash) document.getElementById(`d-${hitHash}`)?.scrollIntoView({ block: "center" });
}

async function renderHash(hash) {
  const rec = await getJson(`api/hash/${hash.toUpperCase()}.json`);
  if (!rec) {
    view.innerHTML = `<p>No disc with content hash <code>${esc(hash.toUpperCase())}</code> is in the database yet.</p>`;
    return;
  }
  await renderSeries(rec.series.slug, rec.contentHash);
}

async function route() {
  const [, kind, arg] = decodeURIComponent(location.hash).match(/^#\/(s|h)\/(.+)$/) ?? [];
  document.title = "AniDiscDB";
  try {
    await loadIndex();
    if (kind === "s") await renderSeries(arg);
    else if (kind === "h") await renderHash(arg);
    else await renderSearch(q.value);
  } catch (err) {
    view.innerHTML = `<p>Could not load the database: ${esc(err.message)}</p>`;
  }
}

let timer;
q.addEventListener("input", () => {
  clearTimeout(timer);
  timer = setTimeout(() => {
    const text = q.value.trim();
    if (isHash(text)) location.hash = `#/h/${text.toUpperCase()}`;
    else if (location.hash && location.hash !== "#/") location.hash = "#/";
    else renderSearch(text);
  }, 120);
});
document.addEventListener("keydown", (e) => {
  if (e.key === "/" && document.activeElement !== q) { e.preventDefault(); q.focus(); }
});
window.addEventListener("hashchange", route);
route();
