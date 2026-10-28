// Tłumaczenie interfejsu PL -> EN. Polski jest językiem źródłowym; angielski nakładany jest na gotowy DOM
// (dokładne dopasowanie tekstu + reguły dla tekstów ze zmiennymi). Brak wpisu = tekst zostaje po polsku.
(() => {
  let lang = "pl";
  try { lang = localStorage.getItem("lang") || "pl"; } catch {}
  if (lang !== "en") return;
  document.documentElement.lang = "en";

  const EXACT = {
    // nawigacja i nagłówki
    "Zadania": "Jobs", "Aktywność": "Activity", "Konta chmurowe": "Cloud accounts", "Ustawienia": "Settings", "Kosz i wersje": "Trash & versions",
    "Wstrzymaj wszystko": "Pause all", "Wznów wszystko": "Resume all", "Zadania synchronizacji": "Sync jobs",
    "Co, dokąd i kiedy ma być synchronizowane.": "What, where and when to synchronize.", "Nowe zadanie": "New job",
    "Rzeczy wymagające uwagi i historia operacji.": "Items that need attention and the operation history.",
    "Wymaga uwagi": "Needs attention", "Historia operacji": "Operation history", "Wszystkie zadania": "All jobs",
    "Szukaj pliku lub operacji…": "Search file or operation…", "Szukaj pliku…": "Search file…",
    "Czas": "Time", "Zadanie": "Job", "Operacja": "Operation", "Plik": "File", "Szczegóły": "Details", "Strona": "Side", "Rozmiar": "Size",
    "Wersja z dnia": "Version from", "Brak wpisów.": "No entries.", "Brak zadań.": "No jobs.", "Wczytywanie…": "Loading…",
    "Kosz i wersje plików": "Trash & file versions", "Pliki usunięte lub nadpisane przez synchronizację. Możesz przywrócić dowolną wersję.": "Files deleted or overwritten by synchronization. You can restore any version.",
    "Opróżnij kosz zadania": "Empty job trash", "Kosz jest pusty.": "Trash is empty.", "lewa": "left", "prawa": "right",
    "Przywróć": "Restore", "Usuń": "Delete", "Usuń trwale?": "Delete permanently?", "Opróżnić kosz?": "Empty trash?", "Opróżnij": "Empty",
    "Kosz opróżniony": "Trash emptied", "Usunięto": "Deleted",
    // konta
    "Po połączeniu konto pojawi się jako lokalizacja": "After connecting, the account appears as a location", "nazwa:": "name:",
    "Dodaj konto": "Add account", "Nazwa konta (np. gdrive)": "Account name (e.g. gdrive)", "Połącz konto": "Connect account",
    "Testuj": "Test", "Brak połączonych kont. Wybierz usługę poniżej.": "No connected accounts. Pick a service below.",
    "Logowanie w przeglądarce": "Sign in via browser", "WebDAV: adres, login, hasło": "WebDAV: address, login, password", "Adres, login, hasło": "Address, login, password",
    "Serwer, login, hasło": "Server, login, password", "Szyfrowanie (crypt)": "Encryption (crypt)", "Szyfruje pliki przed wysłaniem": "Encrypts files before upload",
    "Login": "Login", "Hasło": "Password", "Serwer": "Server", "Port": "Port", "Adres WebDAV": "WebDAV address",
    "Hasło lub hasło aplikacji": "Password or app password", "Hasło szyfrowania": "Encryption password", "Dodatkowe hasło (sól, zalecane)": "Extra password (salt, recommended)",
    "Konto i folder, w którym trafią zaszyfrowane pliki": "Account and folder for the encrypted files",
    "Po kliknięciu „Połącz konto” otworzy się przeglądarka z logowaniem — dokończ je tam.": "After clicking “Connect account” a browser window opens for sign-in — finish it there.",
    "Zapisz hasła w bezpiecznym miejscu — bez nich zaszyfrowanych danych nie da się odzyskać. Konto-źródło musi już istnieć (np. gdrive:Szyfrowane).": "Store the passwords safely — encrypted data cannot be recovered without them. The underlying account must already exist (e.g. gdrive:Encrypted).",
    "Konto dodane": "Account added", "Usunąć konto?": "Delete account?",
    // ustawienia
    "Motyw": "Theme", "Automatyczny": "Automatic", "Jak system": "Like the system", "Jasny": "Light", "Zawsze jasny": "Always light", "Ciemny": "Dark", "Zawsze ciemny": "Always dark",
    "Uruchamiaj razem z systemem": "Start with the system", "Aplikacja startuje w tle, w zasobniku systemowym.": "The app starts in the background, in the system tray.",
    "Powiadomienia systemowe": "System notifications", "O błędach, zatrzymaniu przez zabezpieczenie i konfliktach.": "About errors, safety stops and conflicts.",
    "Język": "Language", "Aktualizacje": "Updates", "Sprawdź aktualizacje": "Check for updates", "Kopia konfiguracji": "Configuration backup",
    "Eksportuj zadania": "Export jobs", "Importuj zadania": "Import jobs", "Plik zawiera zadania i reguły. Hasła i tokeny kont chmurowych nie są eksportowane.": "The file contains jobs and rules. Passwords and cloud tokens are not exported.",
    "Zamknięcie okna chowa aplikację do zasobnika — synchronizacja działa dalej. Zakończ z menu ikony w zasobniku.": "Closing the window hides the app to the tray — synchronization keeps running. Quit from the tray icon menu.",
    "Silnik chmur:": "Cloud engine:", "VSS (Windows): pliki zablokowane przez inne programy": "VSS (Windows): files locked by other programs",
    "Uruchom ponownie jako administrator": "Restart as administrator", "Uruchamiaj przy logowaniu jako administrator": "Start at sign-in as administrator",
    "Tworzy zadanie harmonogramu Windows z najwyższymi uprawnieniami (zastępuje zwykły autostart).": "Creates a Windows scheduled task with highest privileges (replaces the regular autostart).",
    "Sprawdzanie…": "Checking…", "Masz najnowszą wersję.": "You have the latest version.", "Pobieranie…": "Downloading…", "Zainstaluj": "Install",
    "Pobrać i zainstalować teraz? Aplikacja uruchomi się ponownie.": "Download and install now? The app will restart.",
    "Nie udało się sprawdzić aktualizacji (brak opublikowanej aktualizacji lub brak sieci).": "Could not check for updates (no published update or no network).",
    "Działasz jako administrator — pliki zablokowane przez inne programy będą kopiowane z migawki VSS.": "You are an administrator — files locked by other programs will be copied from a VSS snapshot.",
    "Brak uprawnień administratora — zablokowane pliki będą ponawiane, ale VSS nie zadziała.": "No administrator rights — locked files will be retried, but VSS will not work.",
    "Wymyśl": "Suggest",
    // przyciski wspólne
    "Anuluj": "Cancel", "Zamknij": "Close", "OK": "OK", "Dalej": "Next", "Wstecz": "Back", "Zapisz": "Save", "Edytuj": "Edit", "Wyłącz": "Disable", "Włącz": "Enable",
    "Uruchom": "Run", "Symulacja": "Simulate", "Wymuś": "Force", "Liczę…": "Calculating…", "Wybierz folder…": "Choose folder…", "Wczytaj foldery": "Load folders",
    "Konto chmurowe…": "Cloud account…", "Edytuj zadanie": "Edit job", "Zadanie zapisane": "Job saved", "Zadanie usunięte": "Job deleted",
    // kreator
    "Nazwa zadania": "Job name", "Lewa lokalizacja": "Left location", "Prawa lokalizacja": "Right location",
    "Lokalizacją może być folder, dysk sieciowy (": "A location can be a folder, a network drive (", ") albo konto chmurowe dodane w zakładce „Konta chmurowe”.": ") or a cloud account added in the “Cloud accounts” tab.",
    "⇄ Dwustronna": "⇄ Two-way", "Zmiany z obu stron trafiają na drugą": "Changes from both sides go to the other",
    "→ Lewa do prawej": "→ Left to right", "Prawa strona jest kopią lewej": "The right side is a copy of the left",
    "← Prawa do lewej": "← Right to left", "Lewa strona jest kopią prawej": "The left side is a copy of the right",
    "Nie usuwaj po lewej stronie": "Don't delete on the left side", "Nie usuwaj po prawej stronie": "Don't delete on the right side",
    "Usunięcie pliku po prawej nie skasuje go po lewej (w trybie dwustronnym plik wróci na prawą stronę).": "Deleting a file on the right won't delete it on the left (in two-way mode the file returns to the right side).",
    "Usunięcie pliku po lewej nie skasuje go po prawej (w trybie dwustronnym plik wróci na lewą stronę).": "Deleting a file on the left won't delete it on the right (in two-way mode the file returns to the left side).",
    "Tylko dodawaj nowe pliki": "Only add new files", "Nic nie jest nadpisywane ani usuwane — idealne do archiwizacji.": "Nothing is overwritten or deleted — ideal for archiving.",
    "Gdy plik zmieniono po obu stronach (konflikt)": "When a file changed on both sides (conflict)",
    "Nowszy wygrywa": "Newer wins", "Większy wygrywa": "Larger wins", "Lewa strona wygrywa": "Left side wins", "Prawa strona wygrywa": "Right side wins",
    "Zachowaj obie wersje": "Keep both versions", "Pomiń i zapytaj mnie w „Aktywności”": "Skip and ask me in “Activity”",
    "Uwzględnij tylko (maski, jedna na linię)": "Include only (masks, one per line)", "Wyklucz (maski, jedna na linię)": "Exclude (masks, one per line)",
    "Min. rozmiar pliku (MB)": "Min. file size (MB)", "Maks. rozmiar pliku (MB)": "Max. file size (MB)",
    "Wybór podfolderów (lewa strona)": "Subfolder selection (left side)",
    "Zaznacz podfoldery do synchronizacji. Bez zaznaczenia synchronizowane jest wszystko. Zaznaczenia dopisują maski w polu „Uwzględnij tylko”.": "Tick the subfolders to synchronize. With nothing ticked everything is synchronized. Ticks add masks to the “Include only” field.",
    "Najpierw podaj lewą lokalizację (krok 1).": "Enter the left location first (step 1).", "Brak podfolderów.": "No subfolders.",
    "Maski:": "Masks:", "= dowolny ciąg,": "= any string,", "= jeden znak. Pliki tymczasowe (": "= one character. Temporary files (", ") są zawsze pomijane.": ") are always skipped.",
    "Na bieżąco": "Continuously", "Zmiany łapane od razu": "Changes picked up instantly", "Co jakiś czas": "Periodically", "Co N minut": "Every N minutes",
    "Codziennie": "Daily", "O wybranej godzinie": "At a chosen time", "Ręcznie": "Manually", "Tylko na kliknięcie": "Only on click",
    "Okno czasowe: od godziny": "Time window: from hour", "do godziny": "to hour", "Warunki uruchamiania": "Start conditions",
    "Tylko przy zasilaniu sieciowym": "Only on AC power", "Zadanie czeka, gdy komputer pracuje na baterii.": "The job waits while the computer runs on battery.",
    "Tylko w tych sieciach Wi-Fi (jedna nazwa na linię; puste = dowolna)": "Only on these Wi-Fi networks (one name per line; empty = any)",
    "Dodatkowe wyzwalacze": "Additional triggers", "Po podłączeniu dysku": "When a drive is connected",
    "Gdy pojawi się dysk (np. zewnętrzny lub sieciowy), na którym leży jedna z lokalizacji.": "When a drive (e.g. external or network) holding one of the locations appears.",
    "Po wybudzeniu komputera z uśpienia": "After the computer wakes from sleep", "Po zmianie sieci": "After a network change", "Np. po połączeniu z Wi-Fi lub VPN.": "E.g. after connecting to Wi-Fi or a VPN.",
    "Kosz synchronizacji": "Sync trash", "Usunięte i nadpisane pliki trafiają do kosza i można je przywrócić. Wyłącz, aby pliki były usuwane i nadpisywane bez kopii.": "Deleted and overwritten files go to the trash and can be restored. Turn off to delete/overwrite without a copy.",
    "Trzymaj w koszu (dni)": "Keep in trash (days)", "Maks. wersji jednego pliku (0 = bez limitu)": "Max. versions of one file (0 = unlimited)",
    "Sprawdzaj sumę kontrolną po kopiowaniu": "Verify checksum after copying", "Porównuje BLAKE3 źródła i zapisanego pliku (foldery lokalne i sieciowe). Wolniej, ale wykrywa błędy zapisu. Chmury weryfikuje rclone.": "Compares BLAKE3 of the source and the written file (local and network folders). Slower, but detects write errors. Clouds are verified by rclone.",
    "Czytaj pliki zablokowane przez inne programy z migawki VSS": "Read files locked by other programs from a VSS snapshot",
    "Windows: wymaga uruchomienia jako administrator (Ustawienia → VSS).": "Windows: requires running as administrator (Settings → VSS).",
    "Blokada masowego usuwania (% plików)": "Mass-delete guard (% of files)", "Równoległe transfery": "Parallel transfers", "Zadanie włączone": "Job enabled",
    "Jak to będzie działać": "How it will work",
    "Krok 1 z 5 · Lokalizacje": "Step 1 of 5 · Locations", "Krok 2 z 5 · Kierunek i usuwanie": "Step 2 of 5 · Direction & deletion", "Krok 3 z 5 · Filtry i podfoldery": "Step 3 of 5 · Filters & subfolders",
    "Krok 4 z 5 · Harmonogram i warunki": "Step 4 of 5 · Schedule & conditions", "Krok 5 z 5 · Kosz, bezpieczeństwo i podsumowanie": "Step 5 of 5 · Trash, safety & summary",
    "Podaj obie lokalizacje.": "Enter both locations.", "Lewa i prawa lokalizacja nie mogą być takie same.": "Left and right locations can't be the same.",
    "Podaj godzinę w formacie GG:MM, np. 02:30.": "Enter the time as HH:MM, e.g. 02:30.",
    "Co ile minut": "Every how many minutes", "O której godzinie (GG:MM)": "At what time (HH:MM)", "Dodatkowe sprawdzanie co ile minut (dyski sieciowe i chmura)": "Extra check every how many minutes (network drives and cloud)",
    // karty, stany, szablony
    "Aktywne zadania": "Active jobs", "Ostatnia synchronizacja": "Last synchronization", "Stan": "Status",
    "Gotowe": "Ready", "Synchronizuję": "Syncing", "Czeka na pliki": "Waiting for files", "Błąd": "Error", "Zatrzymane": "Stopped", "Wstrzymane": "Paused", "Wyłączone": "Disabled", "Czeka na warunki": "Waiting for conditions",
    "Wszystko w porządku": "All good", "Są sprawy do przejrzenia": "Some items need review", "Skanowanie…": "Scanning…",
    "Lewa": "Left", "Prawa": "Right", "ręcznie": "manual", "na bieżąco": "continuous", "bez usuwania po lewej": "no deletes on the left", "bez usuwania po prawej": "no deletes on the right",
    "tylko dodawanie": "add only", "bez kosza": "no trash", "weryfikacja sum": "checksum verification", "tylko zasilanie sieciowe": "AC power only",
    "Zsynchronizuj pierwsze foldery": "Synchronize your first folders", "Wybierz gotowy szablon albo utwórz zadanie od zera.": "Pick a ready template or create a job from scratch.",
    "Synchronizacja dwustronna": "Two-way synchronization", "Zmiany po obu stronach trafiają na drugą — np. komputer ⇄ NAS.": "Changes on both sides go to the other — e.g. computer ⇄ NAS.",
    "Kopia zapasowa": "Backup", "Lewa → prawa, nic nie jest usuwane po prawej stronie.": "Left → right, nothing is deleted on the right side.",
    "Lustro (dokładna kopia)": "Mirror (exact copy)", "Prawa strona jest identyczna z lewą; usunięte pliki trafiają do kosza.": "The right side is identical to the left; deleted files go to the trash.",
    "Lustro": "Mirror", "Usunąć zadanie?": "Delete job?",
    "Wymusić synchronizację?": "Force synchronization?", "Zachowaj lewą": "Keep left", "Zachowaj prawą": "Keep right", "Konflikt rozstrzygnięty": "Conflict resolved",
    "konflikt": "conflict", "zajęty": "busy", "błąd": "error", "zmieniono po obu stronach": "changed on both sides",
    "Nic nie czeka na Twoją uwagę.": "Nothing needs your attention.", "✓ Nic nie czeka na Twoją uwagę.": "✓ Nothing needs your attention.",
    "kopiuj →": "copy →", "← kopiuj": "← copy", "usuń po lewej": "delete on the left", "usuń po prawej": "delete on the right", "konflikt (zachowaj obie)": "conflict (keep both)", "konflikt (do decyzji)": "conflict (needs decision)",
    "Obie strony są zgodne — nic do zrobienia.": "Both sides match — nothing to do.", "Nic nie zostało zmienione.": "Nothing was changed.",
    "kopiowanie →": "copy →", "kopiowanie ←": "copy ←", "usunięcie": "deletion", "konflikt: obie wersje": "conflict: both versions", "konflikt rozstrzygnięty": "conflict resolved",
    "Zmiany z": "Changes from", "i": "and", "są przenoszone na drugą stronę.": "are carried to the other side.", "Pliki z": "Files from", "są kopiowane do": "are copied to",
    "Limit prędkości transferu": "Transfer speed limit", "Limit (KB/s, 0 = bez limitu)": "Limit (KB/s, 0 = unlimited)", "Limit poza godzinami (KB/s, 0 = bez limitu)": "Limit outside those hours (KB/s, 0 = unlimited)",
    "Limit obowiązuje od godziny": "Limit applies from hour", "Powiadom, gdy zadanie jest niedostępne dłużej niż (minuty, 0 = wyłączone)": "Notify when a job is unavailable for longer than (minutes, 0 = off)",
    "Dziennik i diagnostyka": "Log & diagnostics", "Skopiuj diagnostykę": "Copy diagnostics", "Otwórz folder dziennika": "Open log folder",
    "Diagnostyka zawiera wersję, system, stan zadań (bez ścieżek plików) i ostatnie wpisy dziennika — wklej ją w zgłoszeniu problemu.": "Diagnostics contain the version, system, job states (without file paths) and recent log entries — paste them into a bug report.",
    "Diagnostyka skopiowana do schowka": "Diagnostics copied to clipboard", "Diagnostyka": "Diagnostics", "Zaznacz całość (Ctrl/Cmd+A) i skopiuj.": "Select all (Ctrl/Cmd+A) and copy.",
    "przenieś →": "move →", "← przenieś": "← move", "przeniesienie": "move",
    "Zapisano": "Saved", "jeszcze nie": "not yet", "przed chwilą": "just now", "Jeszcze nie uruchomione": "Not run yet",
  };

  const RULES = [
    [/^Ostatnio (.+)$/, "Last run $1"], [/^(\d+) min temu$/, "$1 min ago"], [/^(\d+) godz\. temu$/, "$1 h ago"],
    [/^skopiowano (\d+), usunięto (\d+)$/, "copied $1, deleted $2"], [/^następne (.+)$/, "next $1"],
    [/^·\s*skopiowano (\d+), usunięto (\d+)$/, "· copied $1, deleted $2"], [/^· następne (.+)$/, "· next $1"],
    [/^kosz (\d+) dni$/, "trash $1 days"], [/^wyklucza (\d+)$/, "excludes $1"], [/^Wi-Fi: (.+)$/, "Wi-Fi: $1"],
    [/^co (\d+) min$/, "every $1 min"], [/^codziennie (\d{2}:\d{2})$/, "daily $1"],
    [/^Uruchomiono: (.+)$/, "Started: $1"], [/^(\d+) plik\(ów\) jest zajętych przez inne programy — ponowię automatycznie\.?$/, "$1 file(s) are in use by other programs — I will retry automatically."],
    [/^(\d+) konflikt\(ów\) czeka na decyzję w zakładce „Aktywność”\.$/, "$1 conflict(s) await a decision in the “Activity” tab."],
    [/^Czeka na warunki:$/, "Waiting for conditions:"], [/^Zatrzymano ze względów bezpieczeństwa:$/, "Stopped for safety reasons:"],
    [/^Symulacja: (.+)$/, "Simulation: $1"], [/^Przeskanowano:$/, "Scanned:"], [/^(\d+) \/ (\d+)$/, "$1 / $2"],
    [/^plików po lewej i$/, "files on the left and"], [/^po prawej\. Planowanych działań:$/, "on the right. Planned actions:"],
    [/^Krok (\d) z (\d)/, "Step $1 of $2"], [/^Wyeksportowano zadań: (\d+)$/, "Exported jobs: $1"], [/^Zaimportowano zadań: (\d+)$/, "Imported jobs: $1"],
    [/^Przywrócono jako: (.+)$/, "Restored as: $1"], [/^Dostępna wersja (.+)$/, "Version $1 available"], [/^Dostępna aktualizacja (.+)$/, "Update $1 available"],
    [/^(\d+) wersji\)?$/, "$1 versions"], [/^\((\d+) wersji\)$/, "($1 versions)"],
    [/^Wersja pliku „(.+)” zostanie usunięta bez możliwości przywrócenia\.$/, "The version of “$1” will be deleted permanently."],
    [/^Zadanie „(.+)” zostanie usunięte\. Pliki na dyskach nie zostaną ruszone\.$/, "Job “$1” will be deleted. Files on disks are not touched."],
    [/^Konto „(.+)” zostanie usunięte z aplikacji \(dane w chmurze zostają\)\.$/, "Account “$1” will be removed from the app (cloud data stays)."],
    [/^Cały kosz zadania „(.+)” \(obie strony\) zostanie usunięty trwale\.$/, "The whole trash of job “$1” (both sides) will be deleted permanently."],
    [/^Kosz tego zadania jest włączony \((\d+) dni.*\)\.$/, "This job's trash is on ($1 days)."],
    [/^Kosz tego zadania jest wyłączony.*$/, "This job's trash is off — nothing new is stored. Turn it on in the job editor (step 5)."],
    [/^Zmiany z (.+) i (.+) są przenoszone na drugą stronę\.$/, "Changes from $1 and $2 are carried to the other side."],
    [/^Pliki z (.+) są kopiowane do (.+)\.$/, "Files from $1 are copied to $2."],
    [/^Uruchamianie: (.+)$/, "Runs: $1"], [/^Usunięte i nadpisane pliki trafiają do kosza na (\d+) dni(.*)\.$/, "Deleted and overwritten files go to the trash for $1 days$2."],
    [/^Kosz wyłączony: usunięte i nadpisane pliki są tracone bez możliwości przywrócenia\.$/, "Trash off: deleted and overwritten files are lost with no way to restore them."],
    [/^Po lewej stronie nic nie jest usuwane\.$/, "Nothing is deleted on the left side."], [/^Po prawej stronie nic nie jest usuwane\.$/, "Nothing is deleted on the right side."],
    [/^Nic nie jest nadpisywane ani usuwane — tylko nowe pliki są dodawane\.$/, "Nothing is overwritten or deleted — only new files are added."],
    [/^Po kopiowaniu sprawdzana jest suma kontrolna pliku\.$/, "A checksum is verified after copying."],
    [/^Zadanie startuje tylko przy zasilaniu sieciowym\.$/, "The job starts only on AC power."],
    [/^Zadanie startuje tylko w sieciach Wi-Fi: (.+)\.$/, "The job starts only on Wi-Fi networks: $1."],
    [/^Dodatkowo uruchamia się: (.+)\.$/, "Also runs: $1."],
  ];

  // Frazy do podmiany wewnątrz dłuższych tekstów (składanych w kodzie z wielu części).
  const PHRASES = [
    [/Czeka na warunki: komputer działa na baterii \(zadanie wymaga zasilania sieciowego\)/g, "Waiting for conditions: the computer is on battery (the job requires AC power)"],
    [/Czeka na warunki: połączono z siecią Wi-Fi „(.+?)”, która nie jest na liście dozwolonych/g, "Waiting for conditions: connected to Wi-Fi “$1”, which is not on the allowed list"],
    [/Ostatnio/g, "Last run"], [/min temu/g, "min ago"], [/godz\. temu/g, "h ago"], [/przed chwilą/g, "just now"], [/skopiowano/g, "copied"], [/usunięto/g, "deleted"],
    [/następne/g, "next"], [/Jeszcze nie uruchomione/g, "Not run yet"], [/konflikty (\d+)/g, "conflicts $1"],
    [/Zatrzymano ze względów bezpieczeństwa:/g, "Stopped for safety reasons:"],
    [/Sprawdź „Symulację”; „Wymuś” pominie blokadę\./g, "Check “Simulate”; “Force” skips the guard."],
    [/jedna ze stron jest pusta, choć wcześniej były tam pliki \(odpięty dysk\?\)/g, "one side is empty although it held files before (unplugged drive?)"],
    [/plan usuwa ponad (\d+)% plików/g, "the plan deletes more than $1% of files"],
    [/Przeskanowano: (\d+) plików po lewej i (\d+) po prawej\. Planowanych działań: (\d+)\./g, "Scanned: $1 files on the left and $2 on the right. Planned actions: $3."],
    [/Zabezpieczenie zatrzymałoby synchronizację:/g, "The guard would stop synchronization:"],
    [/folder niedostępny/g, "folder unavailable"], [/lewa strona:/g, "left side:"], [/prawa strona:/g, "right side:"],
    [/Ten folder nie jest teraz dostępny\. Jeśli to dysk sieciowy, upewnij się, że jest podłączony\./g, "This folder is not available right now. If it is a network drive, make sure it is connected."],
    [/otwarty w innym programie — ponowię automatycznie/g, "open in another program — will retry automatically"],
    [/Działasz jako administrator — pliki zablokowane przez inne programy będą kopiowane z migawki VSS\./g, "You are an administrator — files locked by other programs will be copied from a VSS snapshot."],
    [/Brak uprawnień administratora — zablokowane pliki będą ponawiane, ale VSS nie zadziała\./g, "No administrator rights — locked files will be retried, but VSS will not work."],
    [/plik zajęty/g, "file busy"],
    [/po podłączeniu dysku/g, "after a drive is connected"], [/po wybudzeniu z uśpienia/g, "after waking from sleep"], [/po zmianie sieci/g, "after a network change"],
    [/\bna bieżąco\b/g, "continuously"], [/\bręcznie\b/g, "manually"], [/\bcodziennie (\d)/g, "daily at $1"], [/\bco (\d+) min\b/g, "every $1 min"],
    [/, tylko w godzinach/g, ", only between"], [/\(maks\. (\d+) wersji pliku\)/g, "(max. $1 versions of a file)"],
  ];
  const tr = (str) => {
    const t = str.trim();
    if (!t) return str;
    let out = EXACT[t];
    if (out === undefined) for (const [re, rep] of RULES) if (re.test(t)) { out = t.replace(re, rep); break; }
    let r = out === undefined ? str : str.replace(t, out);
    for (const [re, rep] of PHRASES) r = r.replace(re, rep);
    return r;
  };
  const walk = (root) => {
    const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const nodes = []; for (let n = w.nextNode(); n; n = w.nextNode()) nodes.push(n);
    for (const n of nodes) { const v = tr(n.nodeValue); if (v !== n.nodeValue) n.nodeValue = v; }
    if (root.querySelectorAll) for (const el of root.querySelectorAll("[placeholder],[title]"))
      for (const a of ["placeholder", "title"]) if (el.hasAttribute(a)) { const v = tr(el.getAttribute(a)); if (v !== el.getAttribute(a)) el.setAttribute(a, v); }
  };
  const start = () => {
    walk(document.body);
    new MutationObserver((ms) => {
      for (const m of ms) {
        if (m.type === "characterData") { const v = tr(m.target.nodeValue); if (v !== m.target.nodeValue) m.target.nodeValue = v; }
        else if (m.type === "attributes") { const v = tr(m.target.getAttribute(m.attributeName)); if (v !== m.target.getAttribute(m.attributeName)) m.target.setAttribute(m.attributeName, v); }
        else for (const n of m.addedNodes) { if (n.nodeType === 3) { const v = tr(n.nodeValue); if (v !== n.nodeValue) n.nodeValue = v; } else if (n.nodeType === 1) walk(n); }
      }
    }).observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["placeholder", "title"] });
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start); else start();
  document.title = "SyncPolak";
})();
