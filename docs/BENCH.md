# Benchmarki Syncron (CLI, release bez LTO, 4 rdzenie, dysk kontenera)

Skrypt: `tests/bench/bench.sh [N] [ścieżka CLI]` (TwoWay, pliki tekstowe 1–20 linii, nazwy z polskimi znakami).

## Baza (przed optymalizacjami, runda 1)
| Pliki | pierwszy przebieg | pusty przebieg | jedna zmiana | dry-run | RSS |
|---|---|---|---|---|---|
| 20 000 | 2,87 s | 0,24 s | 0,23 s | 0,12 s | 30 MB |
| 150 000 | 24,82 s | 1,88 s | 4,01 s | 0,96 s | 157 MB |

Obserwacje: pamięć ≈ 1 KB na plik (obie strony); „jedna zmiana" kosztuje ok. 2 s więcej niż pusty przebieg przy 150 tys. plików.

## Po optymalizacji migawki (pusty przebieg nie przepisuje wierszy bazy)
| Pliki | pusty przebieg | jedna zmiana |
|---|---|---|
| 150 000 | 1,18 s (było 1,88 s) | 1,12 s (było 4,01 s) |
