# SyncPolak: plan (80 punktów)

Lekka aplikacja do synchronizacji (Windows + macOS): foldery lokalne, dyski sieciowe, Google Drive, OneDrive, Bitrix24 Drive.

**Stack:** Tauri 2 (Rust + UI Svelte/React), silnik w Rust, SQLite (stan), rclone (chmury), `notify` (watcher), `keyring` (sekrety). Bitrix24 przez WebDAV.

## Etap 1: Fundamenty (1–8)
1. Spisz wymagania funkcjonalne.
2. Persony i 10 scenariuszy użycia.
3. Wybór stacku, ADR.
4. Monorepo: `core/`, `app/`, `ui/`, `docs/`, `tests/`.
5. CI/CD (GitHub Actions) Windows + macOS: clippy, rustfmt, testy.
6. Konwencje: `tracing`, `thiserror`, SemVer.
7. Makiety UI (5 ekranów).
8. Model domenowy: Konto, Zadanie, Reguła, Harmonogram, Wpis stanu, Konflikt.

## Etap 2: Polskie znaki i różnice systemów (9–16)
9. UTF-8 wszędzie, na Windows API szerokie (UTF-16).
10. Normalizacja Unicode: porównania po NFC (macOS bywa NFD), zapis nazwy oryginalnej.
11. Testy dla `ą ć ę ł ń ó ś ź ż` w NFC i NFD.
12. Wielkość liter: wykrywanie kolizji case-insensitive.
13. Niedozwolone znaki i nazwy zastrzeżone Windows (`CON`, `NUL`, `:`, `?`, końcowa kropka/spacja).
14. Długie ścieżki (`\\?\`, manifest).
15. Ścieżki UNC, zmapowane dyski, `/Volumes`.
16. Metadane: tolerancja mtime 2 s, dowiązania, pliki ukryte, `.DS_Store`/`Thumbs.db` ignorowane.

## Etap 3: Wykrywanie zmian (17–24)
17. Watcher zdarzeń (`notify`): ReadDirectoryChangesW / FSEvents.
18. Debouncing 500–2000 ms, scalanie create/modify/rename.
19. Nowe pliki od razu w kolejce, kopiowanie po ustabilizowaniu rozmiaru i mtime.
20. Polling zapasowy dla dysków sieciowych i chmur.
21. Delta/changes chmur: Drive `changes.list`, OneDrive `delta`, WebDAV ETag.
22. Overflow bufora zdarzeń uruchamia skan różnicowy.
23. Wykrywanie rename/move po ID pliku.
24. Skan startowy (catch-up) względem migawki SQLite.

## Etap 4: Pliki otwarte i zablokowane (25–32)
25. Otwieranie z `FILE_SHARE_READ|WRITE|DELETE`.
26. Opcjonalnie VSS dla plików całkowicie zablokowanych.
27. Kolejka ponowień z backoffem; zajęty plik nie blokuje reszty.
28. Pomijanie plików tymczasowych (`~$*`, `.~lock.*`, `*.tmp`, `*.crdownload`, `.part`).
29. Zapis atomowy: plik tymczasowy, weryfikacja, rename.
30. Kontrola spójności źródła (mtime/rozmiar przed i po).
31. Lista „Oczekujące (plik zajęty)” z procesem blokującym (Restart Manager).
32. macOS: Full Disk Access, woluminy sieciowe, kreator uprawnień.

## Etap 5: Silnik i reguły (33–42)
33. Algorytm trójstronny (lewy, prawy, ostatnia migawka).
34. Tryby jednostronne L→R i R→L.
35. Tryb dwustronny z wykrywaniem konfliktów.
36. „Nie usuwaj po lewej” / „nie usuwaj po prawej”.
37. „Nie nadpisuj starszym/nowszym”, „tylko dodawaj”.
38. Strategie konfliktów: nowszy, większy, lewa, prawa, zachowaj obie, zapytaj.
39. Kosz synchronizacji z retencją.
40. Reguły filtrowania: glob, regex, rozszerzenie, rozmiar, data, atrybuty, priorytety.
41. Dry-run (symulacja).
42. Zabezpieczenie przed masowym usunięciem (próg % i puste źródło).

## Etap 6: Konektory (43–50)
43. Interfejs `StorageBackend` + rclone.
44. Google Drive (OAuth2 PKCE, dyski współdzielone, dokumenty Google, limity API).
45. OneDrive / SharePoint (delta, upload sessions).
46. Bitrix24 Drive przez WebDAV.
47. SMB/NAS (UNC, poświadczenia, wznawianie po powrocie sieci).
48. SFTP/WebDAV.
49. Sekrety w Credential Manager / Keychain.
50. Limity transferu, równoległość, wznawianie dużych plików.

## Etap 7: Harmonogram (51–58)
51. Tryb ciągły (real-time).
52. Harmonogram cykliczny z kreatorem.
53. Okna czasowe.
54. Wyzwalacze: start systemu, logowanie, podłączenie dysku, wybudzenie, zmiana sieci.
55. Warunki: zasilanie, nazwa Wi-Fi, łącze taryfowe.
56. Działanie w tle: usługa/Task Scheduler, LaunchAgent.
57. Uruchom / pauza / wznów / zatrzymaj, globalna pauza.
58. Odporność na uśpienie i restart (kolejka w SQLite).

## Etap 8: UI (59–66)
59. Pulpit zadań ze statusami.
60. Kreator zadania w 4 krokach.
61. Wizualny edytor reguł z podglądem na żywo.
62. Widok aktywności i błędów.
63. Widok konfliktów.
64. Tray / pasek menu + powiadomienia.
65. PL/EN, sortowanie `pl`, motywy, dostępność.
66. Dziennik, historia, eksport CSV, diagnostyka.

## Etap 9: Niezawodność i bezpieczeństwo (67–74)
67. Weryfikacja sum (BLAKE3/SHA-256).
68. Wydajność dla ~1 mln plików.
69. Klasyfikacja błędów, retry z jitterem, HTTP 429, brak miejsca.
70. Ochrona przed zapętleniem, wykrywanie nakładających się zadań.
71. Opcjonalne szyfrowanie po stronie klienta, TLS.
72. Minimalne uprawnienia, podpisy, `cargo audit`.
73. Eksport/import konfiguracji, migracje schematu.
74. Telemetria wyłączona domyślnie, logi z rotacją.

## Etap 10: Testy i wydanie (75–80)
75. Testy jednostkowe silnika.
76. Testy integracyjne i chaosowe.
77. Testy ręczne Win 10/11 i macOS (Intel + Apple Silicon).
78. Pakowanie, podpisy, notaryzacja, auto-update.
79. Dokumentacja po polsku.
80. Beta → 1.0, plan v1.x.

## Ryzyka
1. NFC/NFD i wielkość liter → duplikaty i fałszywe usunięcia (etap 2).
2. Utrata danych przez błędną regułę → dry-run, kosz, limit usunięć.
3. Limity API chmur → delta + backoff.
4. Zdarzenia na dyskach sieciowych zawodne → polling zapasowy.
