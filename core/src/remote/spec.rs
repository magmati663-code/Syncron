//! Specyfikacja formularzy kont zdalnych (czyste dane, bez zależności od żadnego zewnętrznego programu).
//! UI buduje formularz dynamicznie z `remote_kinds()`.
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct OptionSpec {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldSpec {
    pub key: String,
    pub label: String,
    /// `text|password|number|select|bool|file|folder`
    pub kind: String,
    pub required: bool,
    pub secret: bool,
    pub default: String,
    pub options: Vec<OptionSpec>,
    pub help: String,
    pub min: Option<i64>,
    pub max: Option<i64>,
    /// Ostrzeżenie pokazywane przy włączeniu pola.
    pub warn: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KindSpec {
    pub kind: String,
    pub label: String,
    pub help: String,
    /// Konto wymaga logowania w przeglądarce (OAuth).
    pub oauth: bool,
    pub fields: Vec<FieldSpec>,
}

fn f(key: &str, label: &str, kind: &str) -> FieldSpec {
    FieldSpec {
        key: key.into(),
        label: label.into(),
        kind: kind.into(),
        required: false,
        secret: kind == "password",
        default: String::new(),
        options: vec![],
        help: String::new(),
        min: None,
        max: None,
        warn: String::new(),
    }
}

impl FieldSpec {
    fn req(mut self) -> Self {
        self.required = true;
        self
    }
    fn def(mut self, d: &str) -> Self {
        self.default = d.into();
        self
    }
    fn help(mut self, h: &str) -> Self {
        self.help = h.into();
        self
    }
    fn warn(mut self, w: &str) -> Self {
        self.warn = w.into();
        self
    }
    fn range(mut self, a: i64, b: i64) -> Self {
        self.min = Some(a);
        self.max = Some(b);
        self
    }
    fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
    fn opts(mut self, o: &[(&str, &str)]) -> Self {
        self.options = o
            .iter()
            .map(|(v, l)| OptionSpec {
                value: (*v).into(),
                label: (*l).into(),
            })
            .collect();
        self
    }
}

fn k(kind: &str, label: &str, help: &str, oauth: bool, fields: Vec<FieldSpec>) -> KindSpec {
    KindSpec {
        kind: kind.into(),
        label: label.into(),
        help: help.into(),
        oauth,
        fields,
    }
}

fn oauth_fields() -> Vec<FieldSpec> {
    vec![f(
        "client_id",
        "Własny identyfikator klienta (Client ID)",
        "text",
    )
    .help("Opcjonalnie. Zostaw puste, aby użyć domyślnej aplikacji Syncron.")]
}

/// Rodzaje kont dostępne w formularzu (kolejność = kolejność w UI).
/// SMB nie jest kontem: użyj ścieżki sieciowej (UNC) lub zamontowanego udziału.
pub fn remote_kinds() -> Vec<KindSpec> {
    let ftp_fields = |tls_def: &str, port_def: &str| {
        vec![
            f("host", "Serwer", "text").req().help("Nazwa lub adres IP, np. ftp.example.com"),
            f("port", "Port", "number").def(port_def).range(1, 65535),
            f("user", "Login", "text").def("anonymous"),
            f("pass", "Hasło", "password"),
            f("tls", "Szyfrowanie (TLS)", "select").def(tls_def).opts(&[
                ("none", "Brak (nieszyfrowane)"),
                ("explicit", "Jawne (explicit, STARTTLS) — zalecane"),
                ("implicit", "Niejawne (implicit, zwykle port 990)"),
            ]).help("Bez TLS hasło i pliki idą przez sieć jawnym tekstem."),
            f("no_check_certificate", "Nie sprawdzaj certyfikatu serwera", "bool").def("false")
                .warn("Ryzykowne: połączenie może zostać podsłuchane lub podmienione. Włącz tylko dla serwera z certyfikatem własnym, któremu ufasz."),
            f("passive", "Tryb pasywny", "bool").def("true").help("Zalecany za zaporą/NAT."),
            f("concurrency", "Maks. równoczesnych połączeń", "number").def("4").range(1, 64),
        ]
    };
    vec![
        k(
            "webdav",
            "WebDAV / Nextcloud",
            "Adres, login i hasło (dla Nextcloud użyj hasła aplikacji).",
            false,
            vec![
                f("url", "Adres WebDAV", "text").req().help("np. https://chmura.example.com/remote.php/dav/files/login/"),
                f("vendor", "Rodzaj serwera", "select").def("other").opts(&[
                    ("nextcloud", "Nextcloud"),
                    ("owncloud", "ownCloud"),
                    ("sharepoint", "SharePoint"),
                    ("other", "Inny"),
                ]),
                f("user", "Login", "text"),
                f("pass", "Hasło lub hasło aplikacji", "password"),
            ],
        ),
        k(
            "sftp",
            "SFTP (SSH)",
            "Hasło albo klucz prywatny SSH. Zalecany plik known_hosts.",
            false,
            vec![
                f("host", "Serwer", "text").req(),
                f("port", "Port", "number").def("22").range(1, 65535),
                f("user", "Login", "text").req(),
                f("pass", "Hasło", "password").help("Zostaw puste, gdy używasz klucza."),
                f("key_file", "Plik klucza prywatnego", "file").help("np. ~/.ssh/id_ed25519"),
                f("key_file_pass", "Hasło do klucza", "password"),
                f("known_hosts_file", "Plik known_hosts", "file")
                    .help("Bez tego pliku tożsamość serwera nie jest weryfikowana (możliwy atak man-in-the-middle)."),
            ],
        ),
        k("ftp", "FTP", "Zwykły FTP lub FTP z TLS.", false, ftp_fields("none", "21")),
        k("ftps", "FTPS (FTP z TLS)", "FTP z szyfrowaniem TLS.", false, ftp_fields("explicit", "21")),
        k(
            "s3",
            "S3 (Amazon i zgodne)",
            "Amazon S3, Wasabi, MinIO, Cloudflare R2, Backblaze B2 (S3) i inne.",
            false,
            vec![
                f("provider", "Dostawca", "select").req().def("AWS").opts(&[
                    ("AWS", "Amazon S3"),
                    ("Wasabi", "Wasabi"),
                    ("MinIO", "MinIO"),
                    ("Cloudflare", "Cloudflare R2"),
                    ("Backblaze", "Backblaze B2 (S3)"),
                    ("Other", "Inny"),
                ]),
                f("access_key_id", "Access Key ID", "text").req().secret(),
                f("secret_access_key", "Secret Access Key", "password").req(),
                f("region", "Region", "text").help("np. eu-central-1 (dla R2: auto)"),
                f("endpoint", "Endpoint", "text").help("Wymagany poza Amazon S3, np. https://s3.eu-central-1.wasabisys.com"),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinds_are_valid() {
        let ks = remote_kinds();
        assert!(ks.iter().all(|k| k.kind != "smb" && k.kind != "crypt"));
        for k in &ks {
            for fl in &k.fields {
                assert!(fl
                    .key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'));
                if fl.kind == "select" {
                    assert!(fl.options.iter().any(|o| o.value == fl.default));
                }
            }
        }
        assert!(ks.iter().any(|k| k.kind == "sftp"));
    }
}
