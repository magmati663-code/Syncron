# Syncron 0.8 — kontrakt zmian (wspólny dla wszystkich zespołów)

Cel: „coś lepszego niż QSync i FreeFileSync”. Poniższe nazwy pól (JSON) są **wiążące**: backend (Rust) i interfejs
(JS) są pisane równolegle przez różne zespoły. Wszystkie nowe pola mają `#[serde(default)]`, więc stare `jobs.json`
działają bez zmian. Nie zmieniaj istniejących nazw pól ani kształtu istniejących poleceń Tauri.

## A. Silnik i model zadania (`core/src/model.rs`, `plan.rs`, `engine.rs`, `location.rs`)

Nowe pola `Job` (nazwa → typ → domyślnie → znaczenie):

| pole | typ | domyślnie | znaczenie |
|---|---|---|---|
| `compare` | `"TimeAndSize"\|"SizeOnly"\|"Content"` | `TimeAndSize` | jak uznać plik za „taki sam”: czas+rozmiar / tylko rozmiar / zawartość (BLAKE3 dla lokalnych; dla kont zdalnych: rozmiar+hash gdy backend go podaje, inaczej czas+rozmiar) |
| `time_tolerance_secs` | `u32` (0..=86400) | `2` | tolerancja czasu modyfikacji (FAT: 2 s) |
| `time_shift_hours` | `u32` (0..=23) | `0` | „ignoruj przesunięcie czasu”: pliki różniące się o całkowitą liczbę godzin ≤ N (zmiana czasu, strefa) są takie same |
| `symlinks` | `"Skip"\|"Follow"` | `Skip` | dowiązania symboliczne: pomijaj (obecnie) albo podążaj przy ODCZYCIE źródła (zapis przez dowiązanie nadal zabroniony) |
| `ignore_hidden` | `bool` | `false` | pomijaj pliki/foldery ukryte (kropka na początku; Windows: atrybut Hidden) |
| `min_age_days` | `Option<u32>` | `None` | pomijaj pliki młodsze niż N dni (zmodyfikowane niedawno) |
| `max_age_days` | `Option<u32>` | `None` | pomijaj pliki starsze niż N dni |
| `detect_moves` | `bool` | `true` | wykrywanie przeniesień/zmian nazwy (obecnie zawsze włączone) |
| `preserve_permissions` | `bool` | `false` | Unix: kopiuj bity uprawnień (chmod); Windows: bez efektu |
| `versioning` | `"Folders"\|"Replace"` | `Folders` | styl wersji w koszu: `Folders` = `.syncron-trash/<data>/ścieżka` (jak dziś), `Replace` = jedna najnowsza kopia na plik |
| `mode` | dodatkowy wariant `"Custom"` | — | tryb „Własny” (jak w FreeFileSync) — używa `custom` |
| `custom` | `Option<CustomRules>` | `None` | reguły trybu `Custom` (patrz niżej) |
| `run_before` | `Option<String>` | `None` | polecenie uruchamiane przed synchronizacją (błąd ≠ 0 anuluje przebieg) |
| `run_after` | `Option<String>` | `None` | polecenie po synchronizacji (zawsze; zmienne środowiskowe `SYNCRON_JOB`, `SYNCRON_COPIED`, `SYNCRON_DELETED`, `SYNCRON_ERRORS`, `SYNCRON_LEFT`, `SYNCRON_RIGHT`) |
| `run_timeout_secs` | `u32` | `300` | limit czasu poleceń (kill po przekroczeniu) |

`CustomRules` = `{ left_only, right_only, left_newer, right_newer, different }`, każde z wartością
`"Skip"|"CopyToRight"|"CopyToLeft"|"DeleteLeft"|"DeleteRight"`:
- `left_only` — plik jest tylko po lewej, `right_only` — tylko po prawej,
- `left_newer` / `right_newer` — oba istnieją, nowszy jest lewy/prawy,
- `different` — oba istnieją, ten sam czas, inny rozmiar/zawartość.
Domyślnie wszystko `Skip`. Zabezpieczenia (blokada masowego usuwania, kosz, `no_delete_*`) działają jak w innych trybach.

Bezpieczeństwo: `run_before`/`run_after` **są czyszczone przy imporcie** (`parse_import`, import CLI) i ustawiane tylko
ręcznie; wykonywane bez powłoki z logowaniem (`sh -c` / `cmd /C` jest dopuszczalne tylko dla pojedynczego polecenia
podanego przez użytkownika w GUI). `Job::validate()` sprawdza zakresy nowych pól.

Przebieg „z wykluczeniami” (podgląd z ręcznymi zmianami): nowa funkcja
`engine::run_job_opts(job, state_dir, RunOpts { dry, force, progress, skip: HashSet<String> })`, gdzie `skip` to klucze
ścieżek (jak w `Report.planned`), których akcji **nie** wykonujemy. Stara `run_job`/`run_job_with` zostają.
Polecenie Tauri: `run_selected(id, skip: Vec<String>)`.

## B. Zdalne protokoły: FTP / FTPS / SFTP / SMB / WebDAV / S3 / … (`location.rs` konta, `main.rs` polecenia kont, UI zakładka „Konta”)

Polecenie `remote_kinds() -> Vec<KindSpec>` zwraca opis formularza (UI buduje formularz dynamicznie):
`KindSpec { kind, label, help, fields: Vec<FieldSpec> }`,
`FieldSpec { key, label, kind: "text"|"password"|"number"|"select"|"bool"|"file"|"folder", required, secret, default, options: Vec<{value,label}>, help }`.

Obsługiwane `kind` (biała lista `REMOTE_KINDS`): `drive`, `onedrive`, `webdav`, `sftp`, `ftp`, `ftps` (ftp z TLS), `smb`,
`s3`, `b2`, `dropbox`, `box`, `pcloud`, `mega`, `crypt`. Dla `sftp`: `host`, `port`(22), `user`, `pass` lub `key_file`
(+ `key_file_pass`), `known_hosts_file`, opcja `use_insecure_cipher` NIE istnieje. Dla `ftp`/`ftps`: `host`, `port`,
`user`, `pass`, `tls` = `none|explicit|implicit`, `no_check_certificate` (checkbox z ostrzeżeniem), `passive`. Dla `smb`:
`host`, `port`, `user`, `pass`, `domain`. Dla `webdav`: `url`, `vendor` (`nextcloud|owncloud|sharepoint|other`), `user`, `pass`.
Hasła/klucze/tokeny nigdy w argv (jak dziś: stdin / `set_remote_keys`).

Nowe polecenia Tauri: `remote_kinds`, `browse_remote(name, path) -> Vec<DirEntry{name,is_dir}>` (`Backend::list_dir`
z limitem czasu i liczby wpisów), `remote_about(name) -> { total, used, free }` (`Backend::about`, opcjonalnie),
`test_remote(name)` (istnieje; dodać zwrot czasu odpowiedzi i wersji serwera, jeśli dostępne).
UI: wybieranie folderu zdalnego (przeglądarka drzewa w kreatorze, krok 1: przycisk „Przeglądaj…” przy ścieżce z `konto:`).

## C. Skalowanie i wygląd interfejsu (`style.css`, `app.js` ustawienia, `index.html`, `main.rs` ustawienia/okno)

Nowe pola `settings.json` (struct `Settings` w `main.rs`, wszystkie `#[serde(default)]`):
`ui_scale: f64` (domyślnie 1.0; 0.75..=2.0, krok 0.05), `font_scale: f64` (1.0; 0.85..=1.5), `density: "compact"|"normal"|"comfortable"`
(`normal`), `sidebar: "auto"|"expanded"|"collapsed"` (`auto`), `reduce_motion: bool`, `high_contrast: bool`,
`window: Option<{x,y,w,h,maximized}>` (zapamiętana pozycja i rozmiar; przywracana przy starcie z przycinaniem do ekranu),
`theme: "auto"|"light"|"dark"` (jeśli już istnieje w UI — zachować zgodność z localStorage).
Skróty: Ctrl+`+` / Ctrl+`-` / Ctrl+`0` / Ctrl+kółko myszy zmieniają `ui_scale`; zmiana zapisywana w ustawieniach.
Układ responsywny: poniżej ~900 px szerokości pasek boczny zwija się do ikon, tabele i karty zawijają się; kreator i okna
dialogowe mają zawsze widoczną stopkę (przycisk „Zapisz/Dalej” nigdy nie wypada poza ekran) przy dowolnej skali.
Zakładka „Ustawienia” → sekcja „Wygląd i skalowanie” z podglądem na żywo i przyciskiem „Przywróć domyślne”.

## D. Interfejs nowych opcji zadania (`index.html` kreator, `app.js` kreator/podgląd)

- Krok 2 („Reguły”): tryb **Własny** z siatką 5 kategorii (lewo/prawo/pomiń/usuń) i objaśnieniami; porównywanie
  (`compare`), tolerancja czasu, przesunięcie czasu.
- Krok 3 („Filtry”): wiek plików (`min_age_days`/`max_age_days`), ukryte, dowiązania; istniejące maski i rozmiary zostają.
- Krok 5 („Zaawansowane”): `detect_moves`, `preserve_permissions`, `versioning`, polecenia przed/po z ostrzeżeniem
  bezpieczeństwa i polem limitu czasu.
- Podgląd (symulacja): tabela akcji z checkboxami „wykonaj”; przycisk „Wykonaj zaznaczone” wywołuje `run_selected`.
- Import ustawień z **FreeFileSync** (`.ffs_gui`/`.ffs_batch`, XML) → polecenie `import_freefilesync(path) -> Vec<Job>`
  (nowa funkcja `core::ffs::parse_ffs_xml`): mapuje pary folderów, wariant (Two way/Mirror/Update/Custom), filtry
  Include/Exclude, wersjonowanie, tolerancję czasu. Zapisane jako wyłączone, bez poleceń.

## Zasady wspólne

- Teksty interfejsu po polsku w kodzie, tłumaczenie EN w `app/ui/i18n.js` (nowe komunikaty = nowe wpisy EXACT/RULES).
- Testy: Rust w `core/tests/`/`cli/tests/` (nazwy `v08_*.rs`), UI: skrypty Playwright w `tests/ui-qa/` (`v08_*.js`).
  Testy zależne od systemu plików: bramkować `cfg(target_os = "linux")` (macOS/Windows: wielkość liter, NFC/NFD, ścieżki).
- `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` (CI ma nowszy Rust niż lokalny: unikaj
  `chunks_exact`, ręcznych `% n == 0`, zbędnych `mut`).
- Dysk jest mały i współdzielony: `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0`, po skończeniu usuń swój `target/`.
- Nie pushuj, nie wydawaj wersji. Commituj tylko na swojej gałęzi worktree.
