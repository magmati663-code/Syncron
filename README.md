# SyncPolak

Lekka aplikacja (Windows + macOS) do synchronizacji folderów, dysków sieciowych (UNC/SMB/NAS) oraz chmur
(Google Drive, OneDrive, Bitrix24 Dysk przez WebDAV) z pełną obsługą polskich znaków. Plan: [PLAN.md](PLAN.md).

## Pobranie
Instalatory buduje GitHub Actions: zakładka **Actions → „Wydanie” → Run workflow** (albo tag `v0.1.0`);
gotowe pliki `.exe`/`.msi` (Windows) i `.dmg` (macOS Apple Silicon i Intel) pojawią się w **Releases**.

Instalatory nie są podpisane certyfikatem, więc przy pierwszym uruchomieniu:
- **Windows:** SmartScreen → „Więcej informacji” → „Uruchom mimo to”.
- **macOS:** prawy przycisk na aplikacji → „Otwórz” (albo `xattr -cr /Applications/SyncPolak.app`). Nadaj dostęp do dysku
  (Ustawienia systemu → Prywatność → Pełny dostęp do dysku), jeśli synchronizujesz foldery chronione.

## Co potrafi
- Kierunki: dwustronna, lewa→prawa, prawa→lewa; osobno „nie usuwaj po lewej/prawej”, „tylko dodawaj nowe”.
- Konflikty: nowszy / większy / lewa / prawa / zachowaj obie wersje / pomiń i pokaż.
- Zmiany wykrywane od razu (ReadDirectoryChangesW / FSEvents) z debouncingiem, plus polling dla dysków sieciowych i chmur,
  skan startowy nadrabiający zmiany z czasu, gdy aplikacja była wyłączona.
- **VSS (Windows):** plik zablokowany wyłącznie przez inny program (Outlook, bazy danych…) jest kopiowany z migawki woluminu.
  Wymaga uprawnień administratora: w Ustawieniach → VSS „Uruchom ponownie jako administrator” (UAC) oraz opcjonalny autostart
  z najwyższymi uprawnieniami. Bez administratora plik trafia do kolejki ponowień, a w aplikacji widać powód.
- Pliki otwarte w innych programach: czytane we współdzielonym trybie (Windows), zapis atomowy (plik tymczasowy → rename),
  kontrola zmiany w trakcie kopiowania, kolejka ponowień z backoffem, zajęty plik nie blokuje reszty.
- Polskie znaki: porównanie po NFC (macOS/NFD), kolizje wielkości liter, nazwy zastrzeżone Windows, długie ścieżki `\\?\`.
- Reguły: maski (uwzględnij/wyklucz), rozmiar min/max; pomijane pliki tymczasowe (`~$*`, `.tmp`, `.DS_Store`, `Thumbs.db`…).
- Zabezpieczenia: symulacja, kosz synchronizacji (domyślnie 30 dni), blokada masowego usuwania i pustej strony.
- Harmonogram: na bieżąco, co N minut, codziennie o godzinie, okno godzin, ręcznie; autostart; praca w zasobniku.
- Historia operacji z eksportem CSV i wyszukiwaniem.
- Interfejs: jasny/ciemny motyw, kreator zadania w 4 krokach z opisem „jak to będzie działać”, szablony (dwustronna, kopia zapasowa, lustro),
  postęp synchronizacji na żywo, ręczne rozstrzyganie konfliktów („Zachowaj lewą/prawą”), podpowiedzi o niedostępnych folderach.

## Chmury
Połączenia z chmurami obsługuje dołączony do instalatora [rclone](https://rclone.org). Konta dodajesz w zakładce
„Konta chmurowe”, a potem wybierasz je jako lokalizację (`gdrive:Folder`).
**Bitrix24:** podaj adres WebDAV swojego Dysku (Bitrix24 → Dysk → „Połącz dysk sieciowy”) oraz login i hasło/hasło aplikacji.

## Rozwój
```
cargo test -p syncpolak-core          # silnik + testy integracyjne
cd app && npm install && npx tauri dev  # aplikacja (wymaga rclone w PATH lub app/src-tauri/bin)
```
Struktura: `core/` (silnik w Rust), `app/src-tauri` (Tauri: komendy, tray), `app/ui` (interfejs HTML/JS).
