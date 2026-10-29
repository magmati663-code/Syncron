# Syncron

Lekka aplikacja (Windows + macOS) do synchronizacji folderów, dysków sieciowych (UNC/SMB/NAS) oraz chmur
(Google Drive, OneDrive, Bitrix24 Dysk przez WebDAV) z pełną obsługą polskich znaków. Plan: [PLAN.md](PLAN.md).

## Pobranie
Instalatory buduje GitHub Actions: zakładka **Actions → „Wydanie” → Run workflow** (albo tag `v0.1.0`);
gotowe pliki `.exe`/`.msi` (Windows) i `.dmg` (macOS Apple Silicon i Intel) pojawią się w **Releases**.

Instalatory nie są podpisane certyfikatem, więc przy pierwszym uruchomieniu:
- **Windows:** SmartScreen → „Więcej informacji” → „Uruchom mimo to”.
- **macOS:** prawy przycisk na aplikacji → „Otwórz” (albo `xattr -cr /Applications/Syncron.app`). Nadaj dostęp do dysku
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
- **Kosz i wersje plików** (można go wyłączyć per zadanie): zakładka „Kosz i wersje” pokazuje usunięte/nadpisane pliki, z limitem dni i liczby
  wersji; przywracanie dowolnej wersji jednym kliknięciem.
- **Warunki i wyzwalacze:** tylko przy zasilaniu sieciowym, tylko w wybranych sieciach Wi-Fi (wykrywanie najlepszym wysiłkiem),
  uruchomienie po podłączeniu dysku, wybudzeniu z uśpienia i zmianie sieci.
- **Weryfikacja sum kontrolnych** (BLAKE3) po kopiowaniu lokalnym; dla kont zdalnych kontrola rozmiaru po wysłaniu oraz porównanie skrótu serwera (MD5/SHA-1/SHA-256), gdy go podaje.
- **Wybór podfolderów** i drzewo różnic w symulacji.
- Ikona w zasobniku zmienia kolor wg stanu (zielona / niebieska / żółta / czerwona / szara); podgląd konfliktu pokazuje rozmiar i czas obu wersji.
- Powiadomienia systemowe, kopia/import konfiguracji, interfejs PL/EN, automatyczne aktualizacje (wymagają klucza: SIGNING.md).
- Historia operacji z eksportem CSV i wyszukiwaniem.
- Interfejs: jasny/ciemny motyw, kreator zadania w 4 krokach z opisem „jak to będzie działać”, szablony (dwustronna, kopia zapasowa, lustro),
  postęp synchronizacji na żywo, ręczne rozstrzyganie konfliktów („Zachowaj lewą/prawą”), podpowiedzi o niedostępnych folderach.

## Linia poleceń i praca bez okna
Do instalatora dołączona jest wersja `syncron` (także jako osobny plik w wydaniu), korzystająca z tego samego folderu danych i zadań:
```
syncron list                         # zadania
syncron run "Dokumenty" --dry-run    # symulacja (bez --dry-run: uruchomienie; --force pomija zabezpieczenia)
syncron daemon                       # działa w tle według harmonogramów (przeładowuje zadania po zmianie w aplikacji)
syncron service install [--boot]     # usługa: logowanie (domyślnie) lub start systemu (Windows, SYSTEM, wymaga admina)
syncron export/import plik.json
```
Kody wyjścia: 0 = ok, 1 = błąd, 2 = konflikty/błędy plików/zablokowanie. Usługa na macOS to agent launchd, na Linuksie usługa systemd użytkownika.

## Pomijanie plików: `.syncignore`
Plik `.syncignore` w głównym folderze któregokolwiek boku (jedna maska na linię, `#` komentarz, `folder/` pomija cały folder) działa dla obu stron i wędruje razem z danymi.

## Konta zdalne (własna warstwa, bez programów zewnętrznych)
Syncron łączy się z serwerami wyłącznie własnym kodem (`core/src/remote/`): WebDAV (Nextcloud, ownCloud, Bitrix24...), FTP/FTPS,
SFTP, S3 oraz Google Drive, OneDrive i Dropbox. Konta dodajesz w zakładce „Konta”, a potem wybierasz je jako lokalizację
(`konto:Folder`, np. `nextcloud:Dokumenty`).

- Dane kont (host, użytkownik...) leżą w `accounts.json` w folderze danych. **Hasła i tokeny nigdy nie trafiają do tego pliku**:
  są w magazynie kluczy systemu (Windows Credential Manager, macOS Keychain, Linux Secret Service). Gdy go nie ma (np. Linux bez D-Bus,
  serwer bez sesji graficznej), sekrety trafiają do pliku `secrets.enc` zaszyfrowanego XChaCha20-Poly1305 losowym kluczem z `secrets.key`
  (uprawnienia 0600). Zmienna `SYNCRON_SECRETS=file` wymusza plik.
- Usunięcie konta usuwa także jego sekrety. Uszkodzony `secrets.enc` jest odkładany jako `*.corrupt-*`, a konta pokazują „wymaga zalogowania”.
- **Migracja z rclone:** jeśli w folderze danych jest stary `rclone.conf`, aplikacja proponuje import (`syncron accounts import-rclone [plik]`).
  Konta WebDAV/FTP/SFTP/S3 przenoszą się razem z hasłami; konta Google Drive/OneDrive/Dropbox trzeba zalogować ponownie.
- CLI: `syncron accounts list|test <nazwa>|delete <nazwa>`.

## Rozwój
```
cargo test -p syncron-core          # silnik + testy integracyjne
cd app && npm install && npx tauri dev  # aplikacja
```
Struktura: `core/` (silnik w Rust), `app/src-tauri` (Tauri: komendy, tray), `app/ui` (interfejs HTML/JS).
