# Syncron – plan 20 rund: wydajność, oszczędność zasobów, niezawodność

Zasady pracy (wynikają z dotychczasowych restartów kontenera i poleceń użytkownika):
- Pracuję sam, sekwencyjnie, **bez równoległych agentów**. Kompilacja: `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=/home/user/target-shared`.
- Każda runda = jedna mała, mierzalna zmiana + test (regresyjny lub pomiarowy) + `cargo fmt`, `clippy -D warnings`, testy dotkniętych crate'ów; pełne `cargo test --workspace` co 4 rundy.
- Commit po każdej rundzie (bez wydania wersji, bez zmiany numeru). Push co kilka rund.
- Żadnej zmiany zachowania bez testu. Priorytet: nie utracić danych > poprawność > szybkość > rozmiar.
- Pomiar przed optymalizacją: jeśli narzut nie jest mierzalny (krytyczna ścieżka, ≥10%), nie ruszam.

## Stan wyjściowy (z kodu)
- Skan: lewa i prawa strona w `thread::scope` (engine.rs), wykonanie akcji w puli wątków (engine.rs ~900).
- Baza: SQLite WAL (engine.rs ~107), migawka zapisywana w jednej transakcji po przebiegu.
- Kopiowanie: bufor 256 KiB, opcjonalny limit pasma i weryfikacja BLAKE3 (location.rs ~1173).
- Runner: wątek na zadanie, `recv_timeout`, watcher z debouncingiem, polling min. 15 s; krótkie `sleep` 25–250 ms w pętlach oczekiwania.
- UI: `setInterval(refreshStatus, 2000)` działa także przy ukrytym oknie.
- Zdalne: ureq (blokujące), suppaftp, russh + tokio (rt-multi-thread), własny SigV4.
- Profil release: `opt-level="s"`, LTO, `codegen-units=1`, `strip`, `panic="abort"`.
- Znane długi z QA: T4-5, T4-10, T5-5, T5-7, T5-8…15, brak testów T3/T6–T10 (hooki, zdalne+przerwanie, aplikacja/CLI/runner).

## Rundy

### Blok A – pomiar i wąskie gardła (rundy 1–4)
1. **Benchmark bazowy**: skrypt `tests/bench/` generujący drzewa (10k / 100k małych plików, 1 plik 2 GB sparse) i mierzący czas skanu, planu, pustego przebiegu, RSS (`/usr/bin/time -v`). Zapis wyników do `docs/BENCH.md`. Bez zmian w kodzie produkcyjnym.
2. **Skan lokalny**: sprawdzić koszt `list_with` (alokacje kluczy NFC, `names::key`, stat na wpis); ograniczyć kopiowanie `String`, użyć `DirEntry::metadata` zamiast osobnego `stat`, tam gdzie to bezpieczne.
3. **Plan i drzewa**: `Tree`=BTreeMap<String,Meta> kopiowane (`tree()` w `run_core`); przejść na referencje/`Arc<str>` kluczy, usunąć zbędne klony `Entry` w `detect_moves` i `equalize_by_content`; test: wynik planu identyczny na fuzzerze z QA (T1).
4. **Baza stanu**: `load_base` i zapis migawki – prepared statements, jedna transakcja, `PRAGMA synchronous=NORMAL` w WAL, `wal_autocheckpoint`, VACUUM tylko przy rotacji historii; test: kill procesu w trakcie zapisu nie psuje bazy (odtworzenie z poprzedniej migawki).

### Blok B – I/O i pamięć (rundy 5–8)
5. **Kopiowanie plików**: adaptacyjny bufor (64 KiB dla małych, 1–4 MiB dla dużych), `copy_file_range`/`CopyFileEx`/`clonefile` przez `fs::copy` tam, gdzie nie trzeba weryfikacji; zachować anulowanie i limit pasma; test: anulowanie w trakcie dużego pliku < 1 s.
6. **Hashowanie**: BLAKE3 z `mmap`/wielowątkowością tylko dla plików > 16 MiB (`update_rayon` bez dodawania rayon: własny podział lub `Hasher::update_mmap`), pamięć podręczna skrótów po (rozmiar, mtime) dla `compare=Content` i `detect_moves`; test T1-01/T1-02 nadal zielone.
7. **Pamięć przy dużych drzewach**: pomiar RSS dla 500k plików; kompaktowe `Entry` (jeden `String` zamiast `real`+klucz gdy są równe), brak trzymania `hashes` dla plików bez potrzeby; docelowo RSS < 250 MB dla 500k plików.
8. **Zdalne – bufory i współbieżność**: ograniczyć liczbę równoległych transferów do konfiguracji (domyślnie 4), ponowne użycie połączeń (keep-alive ureq, jedna sesja SFTP/FTP na wątek roboczy), `tokio` z `current_thread` lub 2 wątkami zamiast `multi_thread` na wszystkich rdzeniach; test: 100 małych plików na backendzie `mem` z wstrzykiwanymi błędami.

### Blok C – runner, procesor, bateria (rundy 9–12)
9. **Runner w bezczynności**: policzyć wybudzenia/s przy 20 zadaniach Realtime; zastąpić krótkie `sleep` w pętlach oczekiwaniem na zmienną warunkową/kanał; test: brak aktywnego oczekiwania (liczba wybudzeń w 10 s poniżej progu).
10. **Watcher**: koalescencja zdarzeń (debounce per zadanie, limit kolejki, obsługa przepełnienia bufora inotify/ReadDirectoryChangesW → pełny skan), ignorowanie `.syncron-trash` i plików tymczasowych; test: 50k zmian w sekundę nie rośnie pamięć.
11. **UI w tle**: `refreshStatus` co 2 s tylko gdy okno widoczne (`document.visibilityState`), w tle co 15 s; zdarzenia Tauri zamiast pollingu tam, gdzie runner już je emituje; test Playwright: brak wywołań `invoke` przy ukrytej karcie.
12. **Start aplikacji**: opóźnione inicjowanie (sprawdzanie aktualizacji po 8 s już jest), leniwe ładowanie `reports.js`, `i18n` reguł, szybkie otwarcie `settings.json`; pomiar czasu do pierwszego malowania w Playwright (budżet < 400 ms na stubie).

### Blok D – niezawodność i odporność (rundy 13–17)
13. **Zapisy atomowe i odzyskiwanie**: wszystkie pliki stanu (`jobs.json`, `settings.json`, `accounts.json`, `secrets.*`) przez jeden `write_atomic` z `fsync` pliku i katalogu, ponowieniem na Windows (antywirus, T5-c); kopia `.bak` poprzedniej wersji; test: obcięcie pliku w losowym miejscu → odtworzenie z `.bak`.
14. **Przerwanie i zasilanie**: test „kill -9 w trakcie przebiegu" (silnik + prawdziwy proces CLI), po restarcie brak plików `.syncron-tmp-*`, brak osieroconych blokad, następny przebieg kończy pracę; sprzątanie starych tmp przy starcie.
15. **Zdalne błędy**: ujednolicić klasyfikację błędów (retry/backoff z jitterem, limity prób, `Retry-After`), timeouty połączenia/odczytu na wszystkich backendach, obsługa zerwanego transferu (wznowienie od offsetu tam, gdzie protokół pozwala, inaczej czyste ponowienie); testy na `mem::Faults`.
16. **Długie przebiegi**: postęp i anulowanie zawsze reagują w < 1 s (także podczas skanu i hashowania); limit czasu skanu zdalnego; ochrona przed pętlą zadań (backoff po powtarzających się błędach, bez zalewania logu); test: zadanie z błędem co przebieg nie rośnie w historii ponad limit.
17. **Długi z QA**: T4-5 (zakotwiczenie masek), T4-10 (zła para nie przerywa importu), T5-5, T5-7, T5-9 (zawężenie uprawnień `secrets.key` do 0600), T5-13/14 (parser rclone.conf), `read_ffs_file` z plikiem specjalnym (limit odczytu przez `take`).

### Blok E – domknięcie (rundy 18–20)
18. **Testy luk**: dopisać brakujące testy dla obszarów, które nie przeszły QA rundy 2 (hooki i webhook, przerywanie przy zdalnych, `edit→run`, CLI `report`/`accounts`), po jednym pliku na raz.
19. **Rozmiar i zależności**: `cargo tree -d` (duplikaty, np. `quick-xml` 0.37/0.42), usunąć nieużywane cechy zależności, `cargo bloat` (jeśli dostępny) dla największych kosztów, porównanie rozmiaru binarki przed/po; nie zmieniać `opt-level` bez pomiaru wydajności.
20. **Regresja końcowa i raport**: pełne `fmt`, `clippy -D warnings`, `cargo test --workspace`, Playwright (`tests/ui-qa`), powtórzenie benchmarków z rundy 1, tabela przed/po w `docs/BENCH.md`, raport dla użytkownika. Decyzja o wydaniu dopiero po zgodzie użytkownika i naprawie rozliczeń CI.

## Kryteria sukcesu
- Brak regresji testów; zero nowych ostrzeżeń clippy.
- Skan 100k plików: ≥ 20 % szybciej niż w rundzie 1; RSS dla 500k plików mierzalnie niższy.
- Runner bezczynny: brak aktywnego oczekiwania; UI w tle: brak pollingu.
- Brak ścieżki, w której awaria/przerwanie zostawia uszkodzony stan lub kasuje dane.
