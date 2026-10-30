# Podpisywanie instalatorów

Budowa (`.github/workflows/release.yml`) podpisuje instalatory automatycznie, gdy w repozytorium są odpowiednie sekrety
(**Settings → Secrets and variables → Actions → New repository secret**). Bez sekretów budowa działa jak dotąd, tylko bez podpisu.

## Windows (usuwa ostrzeżenie SmartScreen)
Potrzebny certyfikat podpisu kodu (OV lub EV) od uznanego wystawcy (np. DigiCert, Sectigo, SSL.com).
Certyfikat EV daje natychmiastową reputację; OV buduje ją z czasem.

| Sekret | Wartość |
|---|---|
| `WINDOWS_CERTIFICATE` | plik `.pfx` w base64: `base64 -w0 cert.pfx` (PowerShell: `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`) |
| `WINDOWS_CERTIFICATE_PASSWORD` | hasło do pliku `.pfx` |

## macOS (usuwa blokadę Gatekeepera)
Potrzebne konto [Apple Developer Program](https://developer.apple.com/programs/) (99 USD/rok) i certyfikat **Developer ID Application**.

| Sekret | Wartość |
|---|---|
| `APPLE_CERTIFICATE` | certyfikat `.p12` w base64: `base64 -i cert.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | hasło użyte przy eksporcie `.p12` |
| `APPLE_SIGNING_IDENTITY` | np. `Developer ID Application: Imię Nazwisko (TEAMID)` |
| `APPLE_ID` | adres e-mail konta Apple Developer |
| `APPLE_PASSWORD` | hasło aplikacji z appleid.apple.com (nie hasło konta) |
| `APPLE_TEAM_ID` | 10-znakowy identyfikator zespołu |

Po dodaniu sekretów uruchom ponownie workflow „Wydanie”: instalatory zostaną podpisane, a aplikacja macOS dodatkowo notaryzowana.

## Automatyczne aktualizacje
Aplikacja sprawdza aktualizacje w wydaniach GitHub (`latest.json`). Klucz publiczny jest w `app/src-tauri/tauri.conf.json`;
do budowy potrzebny jest odpowiadający mu klucz prywatny jako sekret `TAURI_SIGNING_PRIVATE_KEY` (oraz opcjonalnie
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). Bez niego budowa działa, ale aktualizacje się nie publikują.
