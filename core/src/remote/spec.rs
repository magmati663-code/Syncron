//! Opis formularzy kont (rodzaje, pola) dla interfejsu. Czysty opis danych, bez zależności od backendów.
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
}

#[derive(Debug, Clone, Serialize)]
pub struct KindSpec {
    pub kind: String,
    pub label: String,
    pub help: String,
    /// `oauth` = logowanie przez przeglądarkę (przycisk „Zaloguj”), `form` = zwykłe pola.
    pub auth: String,
    pub fields: Vec<FieldSpec>,
}

/// Lista rodzajów kont. Zespół interfejsu (B) uzupełnia pełną specyfikację.
pub fn kinds() -> Vec<KindSpec> {
    Vec::new()
}
