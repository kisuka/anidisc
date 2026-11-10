//! A small HTTPS client. Windows and macOS use the operating system's TLS and certificate store
//! (smaller binaries); Linux uses rustls with bundled roots, so the binary needs no system libraries.

use std::time::Duration;

pub struct Http {
    agent: ureq::Agent,
}

pub enum Fetched {
    Ok(String),
    NotFound,
    Status(u16),
}

impl Http {
    pub fn new() -> Http {
        let builder = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(format!("anidisc/{}", crate::VERSION));
        #[cfg(not(target_os = "linux"))]
        let builder = builder.tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        );
        Http {
            agent: builder.build().into(),
        }
    }

    pub fn get(&self, url: &str) -> Result<Fetched, String> {
        let mut response = self.agent.get(url).call().map_err(|e| e.to_string())?;
        match response.status().as_u16() {
            200..=299 => response
                .body_mut()
                .read_to_string()
                .map(Fetched::Ok)
                .map_err(|e| e.to_string()),
            404 => Ok(Fetched::NotFound),
            code => Ok(Fetched::Status(code)),
        }
    }
}
