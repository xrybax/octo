# Test aktualizacji forka do Octo 2026.10.22.1

Gałąź `feat/rust-m4a-artist-identity` przyjmuje oficjalne wydanie **2026.10.22.1**
z serwerem w Rust. Historia dotychczasowego forka zostaje zachowana, a jako bazę
kodu przyjmujemy aktualne rozwiązania autora. Starych rozszerzeń C# nie przenosimy
w całości. Na tym etapie ponownie wprowadzamy tylko dwie poprawki opisane poniżej.

## 1. Pobieranie AAC/M4A

Shim wybiera `141/140/bestaudio[ext=m4a]` i zachowuje audio AAC z YouTube
bez konwersji do MP3. Zapisywanie tagów kopiuje kodek. Gdy YouTube nie udostępnia
M4A, pobranie zgłasza błąd zamiast zapisać inny format pod rozszerzeniem `.m4a`.
Odtwarzanie podglądu również domyślnie wybiera M4A.

Nowe pobrania używają oficjalnego mechanizmu identyfikacji, tagowania i układania
albumów w bibliotece. Istniejące pliki nie są konwertowane.

## 2. Tożsamość wykonawcy

Nowe wyniki wykonawców mają ID wynikające z ID artysty w Deezerze, zamiast ze
samej nazwy. Dwóch wykonawców „Feel” pozostaje dwiema osobnymi pozycjami. ID jest
przekazywane także z metadanych piosenki i albumu. Strona wykonawcy, dyskografia
i zdjęcie korzystają z wybranego ID, bez zastępowania go popularniejszym imiennikiem.
Pamięć zdjęć również rozróżnia te ID.

Wyszukiwanie wykonawców jest uzupełniane dokładnie pasującymi nazwami z wyników
piosenek. Pomaga to znaleźć artystę pominiętego przez endpoint wyszukiwania artystów.
Jeśli utwór nie ma jeszcze ID wykonawcy, jego tytuł służy do ustalenia tożsamości
przy otwieraniu strony artysty. Zapisane wcześniej ID nadal mogą być odczytywane
z trwałego rejestru.

## Uruchomienie w Portainerze

1. Poczekaj na zielony workflow **Docker Build & Push** dla tej gałęzi. Publikuje
   obrazy dla amd64 i arm64. Samo otwarcie PR nie oznacza jeszcze gotowych obrazów.
2. Utwórz osobny stack z pliku [`deploy/portainer-rust-test.yml`](../deploy/portainer-rust-test.yml).
   Sprawdź adres Navidrome, ścieżkę muzyki i plik zmiennych OpenBao. Oba kontenery
   muszą widzieć muzykę pod `/music`.
3. Testowy Octo będzie pod `http://192.168.1.65:5277`, z osobną konfiguracją
   `/opt/octo-rust-test/config`. Pobieranie zapisuje pliki w tej samej bibliotece
   `/home/master/music`, którą skanuje Navidrome.
4. W Musly lub Resonus dodaj ten adres jako osobne połączenie.

Obrazy:

```text
ghcr.io/xrybax/octo:feat-rust-m4a-artist-identity
ghcr.io/xrybax/octo-yt-dlp-shim:feat-rust-m4a-artist-identity
```

Nowy pakiet `octo-yt-dlp-shim` może początkowo mieć widoczność Private w GitHub
Packages. Jeżeli anonimowe pobranie jest blokowane, ustaw jego widoczność na Public
lub użyj w Portainerze własnego logowania do GHCR.

Shim zawiera już obsługę M4A. Przy zmianie istniejącego stacka zastąp stare
`build:` obrazem powyżej i usuń starą łatkę uruchamianą przez `entrypoint`/`command`.
Pozostaw domyślne uruchamianie nowego obrazu. Stara łatka była przeznaczona do
poprzedniej wersji pliku `app.py`.

Stack testowy korzysta wyłącznie z YouTube i nie uruchamia slskd. Sprawdzanie
oficjalnych aktualizacji jest wyłączone, ponieważ ta gałąź testuje własne obrazy.

## Sprawdzenie działania

- Wyszukaj Feel i otwórz właściwy zespół, potem jego albumy.
- Wyszukaj „A gdy jest już ciemno” i przejdź z utworu do wykonawcy.
- Otwórz także innego wykonawcę o tej samej nazwie: strony i zdjęcia powinny być osobne.
- Pobierz pojedynczy utwór i album: nowe pliki powinny być `.m4a` z kodekiem AAC.
- Sprawdź odtwarzanie w Musly/Resonus i grupowanie pobranego albumu w Navidrome.

Dopiero po tych testach przechodzimy do playlist z polecanymi utworami. Na tej
gałęzi nie dodajemy własnego generatora takich playlist.
