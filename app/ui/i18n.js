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
    // wygląd i skalowanie (v0.8)
    "Wygląd i skalowanie": "Appearance and scaling", "Skala interfejsu": "Interface scale", "Zmniejsz skalę": "Decrease scale", "Zwiększ skalę": "Increase scale",
    "Rozmiar tekstu": "Text size", "Gęstość": "Density", "Zwarta": "Compact", "Więcej treści na ekranie": "More content on screen",
    "Normalna": "Normal", "Domyślny odstęp": "Default spacing", "Wygodna": "Comfortable", "Większe cele dotykowe": "Larger touch targets",
    "Pasek boczny": "Sidebar", "Zwija się w wąskim oknie": "Collapses in a narrow window", "Rozwinięty": "Expanded", "Zawsze z nazwami": "Always with labels",
    "Zwinięty": "Collapsed", "Same ikony": "Icons only", "Ogranicz animacje": "Reduce motion",
    "Wyłącza przejścia i pulsowanie (preferencja systemu „zmniejsz ruch” też jest respektowana).": "Turns off transitions and pulsing (the system “reduce motion” preference is honored too).",
    "Wysoki kontrast": "High contrast", "Mocniejsze obramowania i wyraźniejsze kolory tekstu.": "Stronger borders and clearer text colors.",
    "Przywróć domyślne": "Restore defaults", "Skróty:": "Shortcuts:", "powiększa,": "zooms in,", "pomniejsza,": "zooms out,", "przywraca 100%,": "resets to 100%,",
    "+ kółko myszy zmienia skalę.": "+ mouse wheel changes the scale.",
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
    ") są zawsze pomijane. Wykluczenia możesz też zapisać w pliku": ") are always skipped. You can also keep exclusions in the file",
    "w głównym folderze (jedna maska na linię,": "in the root folder (one mask per line,", "pomija cały folder) — wędruje razem z danymi.": "skips the whole folder) — it travels with the data.",
    "Brak dostępu do folderu": "No access to the folder", "odmowa dostępu": "access denied", "wymaga decyzji": "needs decision",
    "Nie udało się wczytać zadań": "Could not load jobs", "Spróbuj ponownie": "Try again",
    "Nie udało się odczytać listy zadań.": "Could not read the job list.", "Brak odpowiedzi silnika": "Engine not responding",
    "błąd połączenia": "connection error", "Nie uruchomiono: zadanie jest wyłączone lub właśnie się wykonuje.": "Not started: the job is disabled or already running.",
    "Nie udało się odczytać historii.": "Could not read the history.", "Nie udało się odczytać listy kont.": "Could not read the account list.",
    "Nadpisać istniejące konto?": "Overwrite existing account?", "Nadpisz": "Overwrite", "błąd odczytu": "read error",
    "Podaj liczbę całkowitą minut (0 lub więcej); 0 wyłącza powiadomienie.": "Enter a whole number of minutes (0 or more); 0 turns the notification off.",
    "Operacja nie powiodła się — sprawdź wprowadzone wartości.": "The operation failed — check the entered values.",
    "Nie udało się zapisać zadania: dane w formularzu są niepoprawne. Sprawdź wartości liczbowe w krokach 3–5.": "Could not save the job: the form data is invalid. Check the numeric values in steps 3–5.",
    "bez limitu": "no limit",
    "Uwaga: jedna lokalizacja leży wewnątrz drugiej. Synchronizacja kopiowałaby folder sam do siebie — wybierz foldery, które się nie zawierają.": "Warning: one location is inside the other. Sync would copy the folder into itself — choose folders that do not contain each other.",
    "Podaj obie lokalizacje.": "Enter both locations.", "Lewa i prawa lokalizacja nie mogą być takie same.": "The left and right locations cannot be the same.",
    "Podaj godzinę w formacie GG:MM (godzina 0–23, minuty 00–59), np. 02:30.": "Enter a time as HH:MM (hours 0–23, minutes 00–59), e.g. 02:30.",
    "Okno czasowe: podaj obie godziny (od i do) albo zostaw oba pola puste.": "Time window: enter both hours (from and to) or leave both empty.",
    "Godziny limitu: podaj obie (od i do) albo zostaw oba pola puste.": "Limit hours: enter both (from and to) or leave both empty.",
    "Min. rozmiar pliku nie może być większy niż maksymalny.": "Min. file size cannot be larger than the maximum.",
    "plik zajęty": "file in use", "Brak dostępu do folderu": "No access to the folder",
    "Zapisano": "Saved", "jeszcze nie": "not yet", "przed chwilą": "just now", "Jeszcze nie uruchomione": "Not run yet",
    // v08: kreator (tryb własny, porównywanie, filtry, zaawansowane), podgląd, import FreeFileSync
    "Importuj z FreeFileSync…": "Import from FreeFileSync…", "Import z FreeFileSync": "Import from FreeFileSync",
    "Zadania z pliku zostaną zapisane jako wyłączone i bez poleceń. Przejrzyj je i włącz po sprawdzeniu w „Symulacji”.": "Jobs from the file are saved as disabled and without commands. Review them and enable them after checking in “Simulate”.",
    "Zapisz zaznaczone": "Save selected", "Wykonaj zaznaczone": "Run selected", "Akcja": "Action", "Ścieżka": "Path",
    "Zaznacz wszystko": "Select all", "Odznacz wszystko": "Deselect all", "Filtruj ścieżki…": "Filter paths…",
    "Brak pozycji pasujących do filtra.": "No items match the filter.",
    "Odznaczone pozycje zostaną pominięte. „Wykonaj zaznaczone” uruchamia synchronizację tylko z zaznaczonymi akcjami.": "Unticked items will be skipped. “Run selected” runs the synchronization with the ticked actions only.",
    "⚙ Własny": "⚙ Custom", "Osobna akcja dla każdego rodzaju różnicy": "A separate action for each kind of difference",
    "Reguły trybu własnego": "Custom mode rules",
    "Dla każdego rodzaju różnicy wybierz, co zrobić. „Pomiń” zostawia pliki bez zmian. Zabezpieczenia (blokada masowego usuwania, kosz) działają jak w pozostałych trybach.": "For each kind of difference choose what to do. “Skip” leaves the files unchanged. Safeguards (mass-delete guard, trash) work as in the other modes.",
    "Wszystkie reguły to „Pomiń” — to zadanie niczego nie zsynchronizuje.": "All rules are “Skip” — this job will not synchronize anything.",
    "Tylko po lewej": "Left only", "Plik istnieje tylko po lewej stronie.": "The file exists on the left side only.",
    "Tylko po prawej": "Right only", "Plik istnieje tylko po prawej stronie.": "The file exists on the right side only.",
    "Nowszy po lewej": "Newer on the left", "Plik jest po obu stronach, nowszy jest lewy.": "The file is on both sides; the left one is newer.",
    "Nowszy po prawej": "Newer on the right", "Plik jest po obu stronach, nowszy jest prawy.": "The file is on both sides; the right one is newer.",
    "Różne przy tym samym czasie": "Different with the same time", "Ten sam czas modyfikacji, ale inny rozmiar lub zawartość.": "Same modification time but a different size or content.",
    "Pomiń": "Skip", "Kopiuj w prawo →": "Copy to the right →", "← Kopiuj w lewo": "← Copy to the left", "Usuń po lewej": "Delete on the left", "Usuń po prawej": "Delete on the right",
    "Porównywanie plików": "File comparison", "Jak uznać, że plik się zmienił": "How to decide a file has changed",
    "Czas modyfikacji i rozmiar (domyślnie)": "Modification time and size (default)", "Tylko rozmiar": "Size only", "Zawartość pliku (suma kontrolna)": "File content (checksum)",
    "Plik jest taki sam, gdy zgadzają się czas modyfikacji (w tolerancji) i rozmiar. Szybkie — zawartość nie jest czytana.": "A file is the same when modification time (within tolerance) and size match. Fast — content is not read.",
    "Liczy się tylko rozmiar. Najszybsze, ale zmiana pliku bez zmiany rozmiaru zostanie przeoczona.": "Only size counts. Fastest, but a change that keeps the size will be missed.",
    "Porównywana jest zawartość pliku (suma BLAKE3). Najdokładniejsze, ale wolne: każdy plik jest czytany w całości. W chmurze używane są sumy kontrolne dostawcy, jeśli je podaje.": "File content is compared (BLAKE3 checksum). Most accurate but slow: every file is read in full. In the cloud the provider's checksums are used when available.",
    "Tolerancja czasu (sekundy, 0–86400)": "Time tolerance (seconds, 0–86400)", "Ignoruj przesunięcie czasu (godziny, 0–23)": "Ignore time shift (hours, 0–23)",
    "Tolerancja: pliki z czasem różniącym się o mniej niż tyle sekund są takie same (systemy FAT zaokrąglają czas do 2 s). Przesunięcie: pliki różniące się o całkowitą liczbę godzin do podanej wartości (zmiana czasu letni/zimowy, strefa czasowa) są traktowane jako takie same. 0 = wyłączone.": "Tolerance: files whose times differ by less than this many seconds are the same (FAT rounds time to 2 s). Shift: files differing by a whole number of hours up to the given value (daylight saving, time zone) are treated as the same. 0 = off.",
    "Pomijaj pliki młodsze niż (dni)": "Skip files newer than (days)", "Pomijaj pliki starsze niż (dni)": "Skip files older than (days)",
    "Wiek liczony jest od ostatniej modyfikacji pliku. „Młodsze niż” pomija świeżo zmienione pliki (np. otwarte w edytorze), „starsze niż” — stare archiwa.": "Age is counted from the file's last modification. “Newer than” skips freshly changed files (e.g. open in an editor), “older than” skips old archives.",
    "Pomijaj pliki i foldery ukryte": "Skip hidden files and folders", "Nazwy zaczynające się od kropki; w Windows także z atrybutem „Ukryty”.": "Names starting with a dot; on Windows also those with the “Hidden” attribute.",
    "Dowiązania symboliczne": "Symbolic links", "Pomijaj (domyślnie, bezpieczne)": "Skip (default, safe)", "Podążaj za dowiązaniami": "Follow links",
    "Uwaga: podążanie za dowiązaniami odczytuje pliki spoza folderu zadania, a pętle dowiązań mogą zwielokrotnić dane. Zapis przez dowiązania nadal jest zabroniony. Włącz tylko, gdy wiesz, na co wskazują.": "Warning: following links reads files from outside the job folder, and link loops can multiply data. Writing through links is still forbidden. Enable only if you know what they point to.",
    "Styl wersji w koszu": "Trash version style", "Foldery z datą (każda wersja osobno)": "Dated folders (each version kept)", "Zastępuj (tylko najnowsza kopia pliku)": "Replace (latest copy of a file only)",
    "Wykrywaj przeniesienia i zmiany nazwy": "Detect moves and renames", "Przeniesiony plik jest przenoszony po drugiej stronie zamiast kopiowany i usuwany. Wyłącz, jeśli wolisz prostą kopię.": "A moved file is moved on the other side instead of copied and deleted. Turn off if you prefer a plain copy.",
    "Zachowuj uprawnienia plików": "Preserve file permissions", "Linux i macOS: kopiuje bity uprawnień (chmod). W Windows bez efektu.": "Linux and macOS: copies permission bits (chmod). No effect on Windows.",
    "Polecenia przed i po synchronizacji": "Commands before and after synchronization",
    "Uwaga: polecenia są wykonywane z Twoimi uprawnieniami. Nie wklejaj poleceń, których nie rozumiesz ani nieznanych skryptów — mogą skasować lub wysłać dane. Polecenie podane przed synchronizacją, które zakończy się błędem, anuluje przebieg. Pola puste = brak polecenia.": "Warning: commands run with your privileges. Do not paste commands you do not understand or unknown scripts — they can delete or send data. A command before synchronization that fails cancels the run. Empty fields = no command.",
    "Polecenie przed synchronizacją": "Command before synchronization", "Polecenie po synchronizacji": "Command after synchronization",
    "np. /usr/local/bin/zamontuj-dysk": "e.g. /usr/local/bin/mount-drive", "np. /usr/local/bin/powiadom": "e.g. /usr/local/bin/notify",
    "Limit czasu poleceń (sekundy, 1–86400)": "Command time limit (seconds, 1–86400)",
    "Po przekroczeniu limitu polecenie jest przerywane. Po synchronizacji dostępne są zmienne środowiskowe SYNCRON_JOB, SYNCRON_COPIED, SYNCRON_DELETED, SYNCRON_ERRORS, SYNCRON_LEFT, SYNCRON_RIGHT. Importowane zadania nigdy nie zawierają poleceń.": "After the limit the command is killed. After synchronization these environment variables are available: SYNCRON_JOB, SYNCRON_COPIED, SYNCRON_DELETED, SYNCRON_ERRORS, SYNCRON_LEFT, SYNCRON_RIGHT. Imported jobs never contain commands.",
    "Tryb własny: dla każdego rodzaju różnicy obowiązuje osobna reguła.": "Custom mode: a separate rule applies to each kind of difference.",
    "Pliki i foldery ukryte są pomijane.": "Hidden files and folders are skipped.",
    "Dowiązania symboliczne: podążanie za nimi przy odczycie (ostrożnie).": "Symbolic links: followed when reading (be careful).",
    "Wykrywanie przeniesień jest wyłączone: przeniesiony plik zostanie skopiowany od nowa.": "Move detection is off: a moved file will be copied again.",
    "Uprawnienia plików są kopiowane (Linux i macOS).": "File permissions are copied (Linux and macOS).",
    "Przed synchronizacją zostanie uruchomione polecenie; jego błąd anuluje przebieg.": "A command runs before synchronization; its failure cancels the run.",
    "Po synchronizacji zostanie uruchomione polecenie.": "A command runs after synchronization.",
    "Nie udało się zaimportować pliku FreeFileSync — sprawdź, czy to poprawny plik .ffs_gui lub .ffs_batch.": "Could not import the FreeFileSync file — check that it is a valid .ffs_gui or .ffs_batch file.",
    "W pliku nie znaleziono żadnych par folderów do zaimportowania.": "No folder pairs to import were found in the file.",
    "Zaznacz co najmniej jedno zadanie do zapisania.": "Select at least one job to save.",
    "zapis nie powiódł się": "saving failed", "Wybierz plik FreeFileSync": "Choose a FreeFileSync file",
    "Wiek plików: „młodsze niż” nie może być większe niż „starsze niż” — żaden plik nie spełniłby obu warunków.": "File age: “newer than” cannot exceed “older than” — no file could satisfy both conditions.",
    // konta zdalne (v0.8): formularze z remote_kinds, test, przeglądarka folderów
    "Logowanie w przeglądarce": "Sign-in in the browser", "Logowanie w przeglądarce.": "Sign-in in the browser.",
    "Zaloguj przez przeglądarkę": "Sign in via browser", "Czekam na logowanie w przeglądarce…": "Waiting for browser sign-in…", "Łączenie…": "Connecting…",
    "Po kliknięciu „Zaloguj przez przeglądarkę” otworzy się przeglądarka z logowaniem — dokończ je tam.": "After clicking “Sign in via browser” a browser window opens — finish signing in there.",
    "Własny identyfikator klienta (Client ID)": "Custom client ID", "Opcjonalnie. Zostaw puste, aby użyć domyślnej aplikacji Syncron.": "Optional. Leave empty to use the default Syncron app.",
    "OneDrive / SharePoint": "OneDrive / SharePoint", "WebDAV / Nextcloud": "WebDAV / Nextcloud", "SFTP (SSH)": "SFTP (SSH)", "FTPS (FTP z TLS)": "FTPS (FTP over TLS)", "S3 (Amazon i zgodne)": "S3 (Amazon and compatible)",
    "Adres, login i hasło (dla Nextcloud użyj hasła aplikacji).": "Address, login and password (for Nextcloud use an app password).",
    "Adres WebDAV": "WebDAV address", "Rodzaj serwera": "Server type", "Inny": "Other", "Login": "Login", "Hasło lub hasło aplikacji": "Password or app password",
    "Hasło albo klucz prywatny SSH. Zalecany plik known_hosts.": "Password or SSH private key. A known_hosts file is recommended.",
    "Serwer": "Server", "Port": "Port", "Hasło": "Password", "Zostaw puste, gdy używasz klucza.": "Leave empty when using a key.",
    "Plik klucza prywatnego": "Private key file", "Hasło do klucza": "Key password", "Plik known_hosts": "known_hosts file",
    "Bez tego pliku tożsamość serwera nie jest weryfikowana (możliwy atak man-in-the-middle).": "Without this file the server identity is not verified (man-in-the-middle attacks are possible).",
    "Zwykły FTP lub FTP z TLS.": "Plain FTP or FTP with TLS.", "FTP z szyfrowaniem TLS.": "FTP with TLS encryption.",
    "Nazwa lub adres IP, np. ftp.example.com": "Hostname or IP address, e.g. ftp.example.com", "Szyfrowanie (TLS)": "Encryption (TLS)",
    "Brak (nieszyfrowane)": "None (unencrypted)", "Jawne (explicit, STARTTLS) — zalecane": "Explicit (STARTTLS) — recommended", "Niejawne (implicit, zwykle port 990)": "Implicit (usually port 990)",
    "Bez TLS hasło i pliki idą przez sieć jawnym tekstem.": "Without TLS the password and files travel over the network in clear text.",
    "Nie sprawdzaj certyfikatu serwera": "Do not verify the server certificate",
    "Ryzykowne: połączenie może zostać podsłuchane lub podmienione. Włącz tylko dla serwera z certyfikatem własnym, któremu ufasz.": "Risky: the connection can be eavesdropped or tampered with. Enable only for a server with a self-signed certificate that you trust.",
    "Tryb pasywny": "Passive mode", "Zalecany za zaporą/NAT.": "Recommended behind a firewall/NAT.", "Maks. równoczesnych połączeń": "Max. simultaneous connections",
    "Amazon S3, Wasabi, MinIO, Cloudflare R2, Backblaze B2 (S3) i inne.": "Amazon S3, Wasabi, MinIO, Cloudflare R2, Backblaze B2 (S3) and others.",
    "Dostawca": "Provider", "Region": "Region", "Endpoint": "Endpoint", "Wymagany poza Amazon S3, np. https://s3.eu-central-1.wasabisys.com": "Required outside Amazon S3, e.g. https://s3.eu-central-1.wasabisys.com",
    "np. eu-central-1 (dla R2: auto)": "e.g. eu-central-1 (R2: auto)", "np. ~/.ssh/id_ed25519": "e.g. ~/.ssh/id_ed25519",
    "np. https://chmura.example.com/remote.php/dav/files/login/": "e.g. https://cloud.example.com/remote.php/dav/files/login/",
    "Pokaż": "Show", "Ukryj": "Hide", "Pokaż hasło": "Show password", "Wybierz…": "Choose…",
    "Przeglądaj": "Browse", "Przeglądaj…": "Browse…", "Przeglądaj konto": "Browse account", "W górę": "Up", "Gotowe": "Done", "Zamknij": "Close",
    "Wczytywanie…": "Loading…", "Brak podfolderów.": "No subfolders.", "Testuję…": "Testing…", "Błąd połączenia.": "Connection error.",
    "Nie udało się wczytać listy usług.": "Could not load the list of services.", "Połączono.": "Connected.",
    "Udziały SMB/CIFS (dysk sieciowy Windows, Samba, NAS) nie wymagają konta: wpisz ścieżkę sieciową, np.": "SMB/CIFS shares (Windows network drive, Samba, NAS) need no account: enter a network path, e.g.",
    ", albo zamontowany folder, w kreatorze zadania.": ", or a mounted folder, in the job wizard.", "albo zamontowany folder, w kreatorze zadania.": "or a mounted folder, in the job wizard.",
  };

  const RULES = [
    // v08
    [/^Zaznaczono (\d+) z (\d+)(?: · widocznych: (\d+))?$/, (a, b, c) => `Selected ${a} of ${b}` + (c ? ` · visible: ${c}` : "")],
    [/^Wykonano zaznaczone akcje: (\d+)$/, "Ran the selected actions: $1"],
    [/^Tryb: (.+)$/, (m) => "Mode: " + ({ "dwustronna": "two-way", "lewa do prawej": "left to right", "prawa do lewej": "right to left", "własny": "custom" }[m] || m), true],
    [/^Własny — (.+): (.+)\.$/, (c, a) => "Custom — " + ({ "tylko po lewej": "left only", "tylko po prawej": "right only", "nowszy po lewej": "newer on the left", "nowszy po prawej": "newer on the right", "różne przy tym samym czasie": "different with the same time" }[c] || c) + ": " + ({ "pomiń": "skip", "kopiuj w prawo": "copy to the right", "kopiuj w lewo": "copy to the left", "usuń po lewej": "delete on the left", "usuń po prawej": "delete on the right" }[a] || a) + "."],
    [/^Porównywanie: (.+)\.$/, (m) => "Comparison: " + ({ "czas modyfikacji i rozmiar": "modification time and size", "tylko rozmiar (najszybsze, może przeoczyć zmiany)": "size only (fastest, may miss changes)", "zawartość plików (wolniejsze, dokładniejsze)": "file content (slower, more accurate)" }[m] || m) + "."],
    [/^Tolerancja czasu: (\d+) s\.$/, "Time tolerance: $1 s."],
    [/^Ignorowane przesunięcie czasu: do (\d+) godz\.$/, "Ignored time shift: up to $1 h."],
    [/^Pomijane są pliki młodsze niż (\d+) dni\.$/, "Files newer than $1 days are skipped."],
    [/^Pomijane są pliki starsze niż (\d+) dni\.$/, "Files older than $1 days are skipped."],
    [/^Limit czasu poleceń: (\d+) s\.$/, "Command time limit: $1 s."],
    [/^Uzupełnij pole „(.+)”\.$/, "Fill in the field “$1”.", true], [/^Pole „(.+)” poza zakresem \((.*)\)\.$/, "Field “$1” is out of range ($2).", true],
    [/^Nie udało się wczytać folderów: (.+)$/, "Could not load folders: $1", true],
    [/^Ostatnio (.+)$/, "Last run $1"], [/^(\d+) min temu$/, "$1 min ago"], [/^(\d+) godz\. temu$/, "$1 h ago"],
    [/^skopiowano (\d+), usunięto (\d+)$/, "copied $1, deleted $2"], [/^następne (.+)$/, "next $1"],
    [/^·\s*skopiowano (\d+), usunięto (\d+)$/, "· copied $1, deleted $2"], [/^· następne (.+)$/, "· next $1"],
    [/^kosz (\d+) dni$/, "trash $1 days"], [/^wyklucza (\d+)$/, "excludes $1"], [/^Wi-Fi: (.+)$/, "Wi-Fi: $1", true],
    [/^co (\d+) min$/, "every $1 min"], [/^codziennie (\d{2}:\d{2})$/, "daily $1"],
    [/^Uruchomiono: (.+)$/, "Started: $1", true], [/^(\d+) plik\(ów\) jest zajętych przez inne programy — ponowię automatycznie\.?$/, "$1 file(s) are in use by other programs — I will retry automatically."],
    [/^(\d+) konflikt\(ów\) czeka na decyzję w zakładce „Aktywność”\.$/, "$1 conflict(s) await a decision in the “Activity” tab."],
    [/^Czeka na warunki:$/, "Waiting for conditions:"], [/^Zatrzymano ze względów bezpieczeństwa:$/, "Stopped for safety reasons:"],
    [/^Symulacja: (.+)$/, "Simulation: $1", true], [/^Przeskanowano:$/, "Scanned:"], [/^(\d+) \/ (\d+)$/, "$1 / $2"],
    [/^plików po lewej i$/, "files on the left and"], [/^po prawej\. Planowanych działań:$/, "on the right. Planned actions:"],
    [/^Krok (\d) z (\d)/, "Step $1 of $2"], [/^Wyeksportowano zadań: (\d+)$/, "Exported jobs: $1"], [/^Zaimportowano zadań: (\d+)( \(wyłączone — włącz je po sprawdzeniu\))?$/, (n, d) => "Imported jobs: " + n + (d ? " (disabled — enable them after checking)" : "")],
    [/^Przywrócono jako: (.+)$/, "Restored as: $1", true], [/^Dostępna wersja (.+)$/, "Version $1 available"], [/^Dostępna aktualizacja (.+)$/, "Update $1 available"],
    [/^(\d+) wersji\)?$/, "$1 versions"], [/^\((\d+) wersji\)$/, "($1 versions)"],
    [/^Wersja pliku „(.+)” zostanie usunięta bez możliwości przywrócenia\.$/, "The version of “$1” will be deleted permanently.", true],
    [/^Zadanie „(.+)” zostanie usunięte\. Pliki na dyskach nie zostaną ruszone\.$/, "Job “$1” will be deleted. Files on disks are not touched.", true],
    [/^Konto „(.+)” zostanie usunięte z aplikacji \(dane w chmurze zostają\)\.$/, "Account “$1” will be removed from the app (cloud data stays).", true],
    [/^Cały kosz zadania „(.+)” \(obie strony\) zostanie usunięty trwale\.$/, "The whole trash of job “$1” (both sides) will be deleted permanently.", true],
    [/^Kosz tego zadania jest włączony \((\d+) dni.*\)\.$/, "This job's trash is on ($1 days)."],
    [/^Kosz tego zadania jest wyłączony.*$/, "This job's trash is off — nothing new is stored. Turn it on in the job editor (step 5)."],
    [/^Zmiany z (.+) i (.+) są przenoszone na drugą stronę\.$/, "Changes from $1 and $2 are carried to the other side.", true],
    [/^Pliki z (.+) są kopiowane do (.+)\.$/, "Files from $1 are copied to $2.", true],
    [/^Uruchamianie: (.+)$/, "Runs: $1"], [/^Usunięte i nadpisane pliki trafiają do kosza na (\d+) dni(.*)\.$/, "Deleted and overwritten files go to the trash for $1 days$2."],
    [/^Kosz wyłączony: usunięte i nadpisane pliki są tracone bez możliwości przywrócenia\.$/, "Trash off: deleted and overwritten files are lost with no way to restore them."],
    [/^Po lewej stronie nic nie jest usuwane\.$/, "Nothing is deleted on the left side."], [/^Po prawej stronie nic nie jest usuwane\.$/, "Nothing is deleted on the right side."],
    [/^Nic nie jest nadpisywane ani usuwane — tylko nowe pliki są dodawane\.$/, "Nothing is overwritten or deleted — only new files are added."],
    [/^Po kopiowaniu sprawdzana jest suma kontrolna pliku\.$/, "A checksum is verified after copying."],
    [/^Zadanie startuje tylko przy zasilaniu sieciowym\.$/, "The job starts only on AC power."],
    [/^Zadanie startuje tylko w sieciach Wi-Fi: (.+)\.$/, "The job starts only on Wi-Fi networks: $1.", true],
    [/^Dodatkowo uruchamia się: (.+)\.$/, "Also runs: $1."],
    [/^(.+): odmowa dostępu$/s, "$1: access denied", true],
    [/^Synchronizuję \((\d+)\)$/, "Syncing ($1)"],
    [/^\\\\serwer\\udział lub onedrive:$/, "\\\\server\\share or onedrive:", true],
    [/^Limit prędkości: (.+?) KB\/s w godzinach (.+), poza nimi (.+?) KB\/s\.$/, (a, h, b) => `Speed limit: ${a === "bez limitu" ? "unlimited" : a + " KB/s"} between ${h}, outside those hours: ${b === "bez limitu" ? "unlimited" : b + " KB/s"}.`, true],
    [/^Limit prędkości: (.+?) KB\/s\.$/, "Speed limit: $1 KB/s.", true],
    [/^limit (.+?) KB\/s$/, "limit $1 KB/s", true],
    [/^\\\\serwer\\udział$/, "\\\\server\\share", true],
    [/^(Nie udało się wczytać zadań|Nie udało się zmienić stanu wstrzymania|Nie udało się wczytać historii|Nie udało się wczytać kont): (.*)$/s, (a, b) => ({ "Nie udało się wczytać zadań": "Could not load jobs", "Nie udało się zmienić stanu wstrzymania": "Could not change the pause state", "Nie udało się wczytać historii": "Could not load history", "Nie udało się wczytać kont": "Could not load accounts" })[a] + ": " + b, true],
    [/^Brak odpowiedzi silnika synchronizacji — widoczny stan może być nieaktualny: (.*)$/s, "The sync engine is not responding — the displayed status may be outdated: $1", true],
    [/^Nie udało się wczytać ustawień — widoczne wartości są domyślne i nie zostaną zapisane: (.*)$/s, "Could not load settings — the values shown are defaults and will not be saved: $1", true],
    [/^Pokazano najnowsze (\d+) wpisów \(limit widoku\) — starsze są pominięte\. Eksport CSV obejmuje tylko wpisy widoczne po filtrze\.$/, "Showing the latest $1 entries (view limit) — older ones are omitted. The CSV export covers only the entries visible after filtering."],
    [/^Konto „(.+)” już istnieje\. Dodanie konta pod tą samą nazwą nadpisze jego konfigurację\. Aby tego uniknąć, zmień nazwę\.$/, "Account “$1” already exists. Adding an account with the same name will overwrite its configuration. Change the name to avoid this.", true],
    [/^(.+): podaj liczbę całkowitą od (\S+) do (\S+)\.$/, (l, a, b) => ({"Odstęp w minutach":"Interval in minutes","Okno czasowe, od godziny":"Time window from hour","Okno czasowe, do godziny":"Time window to hour","Blokada masowego usuwania":"Mass-delete guard","Równoległe transfery":"Parallel transfers","Dni w koszu":"Days in trash","Maks. wersji pliku":"Max. versions per file","Limit prędkości":"Speed limit","Limit poza godzinami":"Off-hours limit","Limit od godziny":"Limit from hour","Limit do godziny":"Limit to hour","Min. rozmiar pliku":"Min. file size","Maks. rozmiar pliku":"Max. file size","Tolerancja czasu":"Time tolerance","Przesunięcie czasu":"Time shift","Wiek: młodsze niż (dni)":"Age: newer than (days)","Wiek: starsze niż (dni)":"Age: older than (days)","Limit czasu poleceń":"Command time limit"}[l] || l) + ": enter a whole number from " + a + " to " + b + ".", true],
    [/^(.+): podaj liczbę MB od 0 do ([\d\s\u00a0]+) \(puste lub 0 = bez limitu\)\.$/, (l, n) => ({"Odstęp w minutach":"Interval in minutes","Okno czasowe, od godziny":"Time window from hour","Okno czasowe, do godziny":"Time window to hour","Blokada masowego usuwania":"Mass-delete guard","Równoległe transfery":"Parallel transfers","Dni w koszu":"Days in trash","Maks. wersji pliku":"Max. versions per file","Limit prędkości":"Speed limit","Limit poza godzinami":"Off-hours limit","Limit od godziny":"Limit from hour","Limit do godziny":"Limit to hour","Min. rozmiar pliku":"Min. file size","Maks. rozmiar pliku":"Max. file size"}[l] || l) + ": enter a size in MB from 0 to " + n.replace(/[\s\u00a0]+/g, ",") + " (empty or 0 = no limit).", true],
  ];

  // ---- v0.8, zespół E: raporty, powiadomienia, webhook (nie nadpisuje istniejących wpisów) ----
  const EXACT_E = {
    "Podsumowanie": "Summary", "Ostatnie 24 godziny": "Last 24 hours", "Ostatnie 7 dni": "Last 7 days", "Ostatnie 30 dni": "Last 30 days", "Ostatnie 90 dni": "Last 90 days",
    "Cała historia": "All history", "Eksportuj raport…": "Export report…", "Eksportuj raport": "Export report", "Zakres podsumowania": "Summary range",
    "Przebiegi": "Runs", "Skopiowane": "Copied", "Usunięte": "Deleted", "Konflikty": "Conflicts", "Błędy": "Errors", "Najczęstsze błędy:": "Most common errors:",
    "Powiadomienia i webhook": "Notifications and webhook", "Powiadomienia systemowe": "System notifications",
    "Tylko błędy i konflikty": "Errors and conflicts only", "Po każdym przebiegu": "After every run", "Nigdy": "Never", "Wysyłaj webhook": "Send webhook",
    "Adres webhooka (http lub https)": "Webhook address (http or https)", "Wklej adres (zaczyna się od http lub https)": "Paste the address (starts with http or https)",
    "Format wiadomości": "Message format", "JSON (ogólny)": "JSON (generic)", "ntfy (tekst)": "ntfy (text)",
    "Zezwól na adresy lokalne": "Allow local addresses",
    "Np. serwer ntfy w sieci domowej. Domyślnie adresy lokalne i prywatne są blokowane.": "E.g. an ntfy server on your home network. Local and private addresses are blocked by default.",
    "Wyślij testowy": "Send test",
    "Prywatność: adres webhooka bywa tajny (często zawiera token). Jest zapisany w konfiguracji zadania i trafia do pliku eksportu, ale nigdy nie jest wczytywany przy imporcie ani wypisywany w dzienniku. Do usługi wysyłana jest nazwa zadania, liczby plików, lokalizacje i komunikaty błędów (mogą zawierać nazwy plików).":
      "Privacy: the webhook address is often secret (it usually contains a token). It is stored in the job configuration and included in exported files, but it is never read on import or written to the log. The service receives the job name, file counts, locations and error messages (which may contain file names).",
    "Format": "Format", "Zakres dat": "Date range", "CSV (Excel, separator „;”)": "CSV (Excel, “;” separator)", "HTML (samodzielny plik)": "HTML (standalone file)",
    "Własny zakres…": "Custom range…", "Od": "From", "Do": "To", "Rodzaj operacji": "Operation type", "Wszystkie": "All", "Kopiowanie": "Copying", "Usunięcia": "Deletions",
    "Przeniesienia": "Moves", "Raport CSV chroni przed wstrzyknięciem formuł (pola zaczynające się od „=”, „+”, „-”, „@” dostają apostrof). Raport HTML nie ładuje żadnych zewnętrznych zasobów.":
      "The CSV report protects against formula injection (fields starting with “=”, “+”, “-”, “@” get an apostrophe). The HTML report loads no external resources.",
    "Zapisz raport…": "Save report…",
    "Podaj datę początkową lub końcową albo wybierz gotowy zakres.": "Enter a start or end date, or pick a preset range.", "Niepoprawna data.": "Invalid date.",
    "Data początkowa nie może być późniejsza niż końcowa.": "The start date cannot be later than the end date.",
    "Nie udało się wczytać podsumowania.": "Could not load the summary.", "Nie udało się zapisać raportu.": "Could not save the report.",
    "Adres webhooka musi zaczynać się od http:// lub https://, zawierać nazwę hosta i nie może mieć loginu ani hasła.": "The webhook address must start with http:// or https://, contain a host name and have no user name or password.",
    "Podaj adres webhooka albo wybierz „Nigdy” w polu „Wysyłaj webhook”.": "Enter a webhook address or choose “Never” in “Send webhook”.",
    "Najpierw podaj adres webhooka.": "Enter the webhook address first.",
    "Adres musi zaczynać się od http:// lub https:// i nie może mieć loginu ani hasła.": "The address must start with http:// or https:// and have no user name or password.",
    "\\\\serwer\\udział lub nas:": "\\\\server\\share or nas:",
    "Wysyłam…": "Sending…", "Wysłano wiadomość testową.": "Test message sent.", "Nie udało się wysłać wiadomości testowej.": "Could not send the test message.",
  };
  for (const [k, v] of Object.entries(EXACT_E)) if (!(k in EXACT)) EXACT[k] = v;
  const WH_EN = { "wyłączone": "off", "tylko błędy i konflikty": "errors and conflicts only", "po każdym przebiegu": "after every run", "wyłączony": "off", "tylko przy błędach i konfliktach": "on errors and conflicts only" };
  RULES.push(
    [/^Zapisano raport: (\d+) wpisów$/, "Report saved: $1 entries"],
    [/^Powiadomienia systemowe: (.+)\.$/, (m) => "System notifications: " + (WH_EN[m] || m)],
    [/^Webhook \((.+)\): (.+)\.$/, (f, m) => `Webhook (${f}): ` + (WH_EN[m] || m) + ".", true],
  );

  // Frazy do podmiany wewnątrz dłuższych tekstów (składanych w kodzie z wielu części).
  const PHRASES = [
    [/Czeka na warunki: komputer działa na baterii \(zadanie wymaga zasilania sieciowego\)/g, "Waiting for conditions: the computer is on battery (the job requires AC power)"],
    [/Czeka na warunki: połączono z siecią Wi-Fi „(.+?)”, która nie jest na liście dozwolonych/g, "Waiting for conditions: connected to Wi-Fi “$1”, which is not on the allowed list"],
    [/Ostatnio/g, "Last run"], [/min temu/g, "min ago"], [/godz\. temu/g, "h ago"], [/przed chwilą/g, "just now"], [/skopiowano/g, "copied"], [/usunięto/g, "deleted"], [/przeniesiono/g, "moved"],
    [/następne/g, "next"], [/Jeszcze nie uruchomione/g, "Not run yet"], [/konflikty (\d+)/g, "conflicts $1"],
    [/Zatrzymano ze względów bezpieczeństwa:/g, "Stopped for safety reasons:"],
    [/plan usuwa ponad (\d+)% plików/g, "the plan deletes more than $1% of files"],
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
    [/plik zajęty/g, "file busy"], [/Lewa:/g, "Left:"], [/Prawa:/g, "Right:"], [/\(nowsza\)/g, "(newer)"], [/\(większa\)/g, "(larger)"],
    [/po podłączeniu dysku/g, "after a drive is connected"], [/po wybudzeniu z uśpienia/g, "after waking from sleep"], [/po zmianie sieci/g, "after a network change"],
    [/\bna bieżąco\b/g, "continuously"], [/\bręcznie\b/g, "manually"], [/\bcodziennie (\d)/g, "daily at $1"], [/\bco (\d+) min\b/g, "every $1 min"],
    [/, tylko w godzinach/g, ", only between"], [/, jako jedna najnowsza kopia pliku/g, ", as a single latest copy of the file"], [/\(maks\. (\d+) wersji pliku\)/g, "(max. $1 versions of a file)"],
  ];
  // --- Ochrona danych użytkownika ---------------------------------------------------------------------
  // Nazwy zadań, ścieżki, nazwy plików i kont NIE są tłumaczone. Element (lub potomek elementu) z atrybutem data-nt
  // oraz znane kontenery danych są pomijane. PROT_IN: pomijany jest cały element i jego potomkowie; PROT_OWN: tylko
  // teksty bezpośrednio w elemencie (potomne <span class="kind"> itp. nadal są tłumaczone).
  const PROT_IN = "[data-nt], .card .name, .loc .t span, .treerow span, #repBody summary, " +
    "#hist td:nth-child(2), #hist td:nth-child(4), #pending td:nth-child(2), #pending td[colspan='3'], #trashTbl td:first-child > b, " +
    "#remotes .grow > b, #summary li b, .cur, #histJob option:not([value='']), #trashJob option:not([value='']), select[data-remote] option:not([value=''])";
  const PROT_OWN = "#repBody .scroll div, #repBody details div";
  // Frazy (PHRASES) tylko w kontenerach z komunikatami aplikacji, nigdy w ogólnym tekście.
  const MSG = ".msg.warn, .meta, .stats, .pill, #stepName, #summary li, #pending td.muted, #hist td.muted, #repBody > p, #trashTbl .muted, .tile .v";
  const protectedEl = (el) => !!el && (!!el.closest(PROT_IN) || el.matches(PROT_OWN));

  // Reguły zaczynające się stałym słowem trafiają do kubełka wg pierwszej litery (zamiast ~65 regexów na każdy tekst).
  const BUCKET = {}, ANY = [];
  for (const r of RULES) { const m = /^\^([^\\[(.?*+|^$])/.exec(r[0].source); if (m) (BUCKET[m[1]] ||= []).push(r); else ANY.push(r); }
  const cache = new Map();
  const tr = (str, msg) => {
    const t = str.trim();
    if (!t) return str;
    const key = (msg ? "1" : "0") + str;
    const hit = cache.get(key);
    if (hit !== undefined) return hit;
    let out = EXACT[t], phrases = msg;
    if (out === undefined) for (const list of [BUCKET[t[0]] || [], ANY]) {
      for (const [re, rep, noPhr] of list) {
        const m = re.exec(t);
        if (m) { out = typeof rep === "function" ? rep(...m.slice(1)) : t.replace(re, rep); if (noPhr) phrases = false; break; }
      }
      if (out !== undefined) break;
    }
    let r = out === undefined ? str : str.replace(t, () => out);   // funkcja: bez interpretacji $&, $' itd.
    if (phrases) for (const [re, rep] of PHRASES) r = r.replace(re, rep);
    if (cache.size > 4000) cache.clear();
    cache.set(key, r);
    return r;
  };
  const fixText = (n) => {
    const val = n.nodeValue;
    if (!val || !val.trim()) return;
    // Najpierw tanie sprawdzenie: jeśli tekst nie zmienia się w żadnym trybie, nie dotykamy DOM (dane użytkownika zwykle tu odpadają).
    const plain = tr(val, false), full = tr(val, true);
    if (plain === val && full === val) return;
    const el = n.parentElement; if (!el || el.tagName === "TEXTAREA" || protectedEl(el)) return;
    const v = full !== plain && el.closest(MSG) ? full : plain;
    if (v !== val) n.nodeValue = v;
  };
  const fixAttrs = (el) => {
    if (protectedEl(el)) return;
    for (const a of ["placeholder", "title"]) { const o = el.getAttribute(a); if (o) { const v = tr(o, false); if (v !== o) el.setAttribute(a, v); } }
  };
  const walk = (root) => {
    if (root.nodeType === 3) { fixText(root); return; }
    const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const nodes = []; for (let n = w.nextNode(); n; n = w.nextNode()) nodes.push(n);
    for (const n of nodes) fixText(n);
    if (root.matches && root.matches("[placeholder],[title]")) fixAttrs(root);
    if (root.querySelectorAll) for (const el of root.querySelectorAll("[placeholder],[title]")) fixAttrs(el);
  };
  window.i18nTr = (str) => tr(String(str), false);   // dla wyszukiwarki: dopasowanie do widocznego (przetłumaczonego) tekstu
  const start = () => {
    walk(document.body);
    const obs = new MutationObserver((ms) => {
      for (const m of ms) {
        if (m.type === "characterData") fixText(m.target);
        else if (m.type === "attributes") fixAttrs(m.target);
        else for (const n of m.addedNodes) if (n.nodeType === 3 || n.nodeType === 1) walk(n);
      }
      obs.takeRecords();   // własne zmiany nie wywołują kolejnych przebiegów
    });
    obs.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["placeholder", "title"] });
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start); else start();
  document.title = "Syncron";
})();
