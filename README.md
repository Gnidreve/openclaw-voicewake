# openclaw-voicebridge

Lokaler Sprachdienst für macOS (Apple Silicon):

```
Wake Word → Mikrofon → VAD/Stille-Erkennung → WAV/PCM → whisper-cli
   → OpenClaw-CLI → Antworttext → Piper TTS → Mac-Lautsprecher
```

Alles läuft lokal. Keine Cloud-API, kein Docker, keine dauerhafte
Audioaufzeichnung.

## Zustandsmaschine

```
IDLE → LISTENING_FOR_WAKEWORD → RECORDING → TRANSCRIBING
     → SENDING_TO_OPENCLAW → SPEAKING → IDLE
                                 │
                                 └──→ RECORDING (Folgeeingabe, kein Wake-Word nötig)
```

Jeder Zwischenzustand kann bei Fehlern/Timeouts direkt zurück nach `IDLE`
springen, damit der Dienst nicht hängen bleibt. Während `SPEAKING` läuft
keine Wake-Word-Erkennung (sie wird nur im Zustand
`LISTENING_FOR_WAKEWORD` gestartet) - die eigene TTS-Ausgabe kann sich
also nicht selbst erneut triggern.

Nach einer vorgelesenen Antwort bleibt der Kanal offen: `SPEAKING` springt
direkt zurück nach `RECORDING` und spielt dabei den Start-Ton (siehe
[Bestätigungstöne](#bestätigungstöne)), sodass eine Folgeeingabe ohne
erneutes Wake-Word möglich ist. Eine Runde ist dabei genau **eine**
Funktion (`run_round`), die für beide Auslöser identisch läuft - Wake-Word
und Folgerunde unterscheiden sich nur darin, wer sie aufruft, nicht in
ihrem Ablauf. Beim Debuggen taugt die erste Runde deshalb als Referenz für
alle weiteren. Bleibt diese Folgeaufnahme ohne
erkannte Sprache (leeres Transkript) oder liefert OpenClaw keine Antwort,
geht der Dienst nach `IDLE` zurück - ab dann ist wieder das Wake-Word
nötig. Dass der Kanal zu ist, hört man daran, dass kein neuer Start-Ton
mehr kommt.

Der offene Kanal ist zusätzlich hart begrenzt: nach
`conversation.max_followup_turns` Folgerunden (Standard: 3) wird er auch
dann geschlossen, wenn jede Runde eine Antwort erzeugt hat. Ohne diese
Grenze können Fremdgeräusche im Raum - im Feldtest ein laufender Fernseher -
den Kanal beliebig lange offen halten, weil jede "Eingabe" wieder eine
Antwort erzeugt. Mit `max_followup_turns = 0` ist nach jeder Antwort sofort
wieder das Wake-Word nötig.

## Wann eine Aufnahme endet

Es gibt genau **eine** Uhr: `vad.silence_timeout_ms` (Standard 4000 ms).
Sie läuft ab dem ersten Frame und wird von jedem Frame über
`vad.silence_rms_threshold` wieder auf null gesetzt. Läuft sie ab, endet
die Aufnahme.

Das gilt bewusst unabhängig davon, ob überhaupt jemand gesprochen hat:

- **Niemand spricht** → die Uhr läuft nach 4 s ab, die Aufnahme endet,
  `speech_started` ist `false` → kein Absende-Ton, Whisper wird nicht
  aufgerufen, es geht nichts an OpenClaw.
- **Jemand spricht** → jedes Sprach-Frame setzt die Uhr zurück, die
  Aufnahme läuft weiter. 4 s nach dem letzten Wort endet sie.

Sprechen *verzögert* das Ende also nur; es ist kein zweiter Mechanismus.
Derselbe Wert ist damit sowohl die Zeit, die man nach dem Wake-Word zum
Anfangen hat, als auch die Pause, die eine Äußerung beenden darf.

`vad.max_recording_seconds` ist **keine** Längenbegrenzung für Sprache,
sondern ein Sicherheitsnetz: Liegt die Umgebungslautstärke dauerhaft über
dem Schwellwert (laufender Fernseher), läuft die Uhr nie ab und der
Aufnahmepuffer würde unbegrenzt wachsen. Der Standard (300 s) liegt weit
über jeder realistischen Äußerung, `0` schaltet das Netz ganz ab.

> Früher setzte der Stille-Timeout erkannte Sprache voraus. Eine Runde
> ohne jede Sprache konnte deshalb gar nicht regulär enden und lief in den
> damaligen 60-Sekunden-Deckel - eine Minute, bevor überhaupt auffiel,
> dass nichts angekommen war.

## Voraussetzungen

- macOS auf Apple Silicon
- Rust (stable, via [rustup](https://rustup.rs))
- `ffmpeg` (`brew install ffmpeg`)
- `whisper.cpp` mit einem `whisper-cli`-Binary im PATH (oder Pfad in
  `config.toml` eintragen) und dem Modell unter dem konfigurierten Pfad,
  standardmäßig:
  `/Users/mac-mini/.openclaw/workspace/state/whisper.cpp-model-large-v3-turbo/ggml-large-v3-turbo.bin`
- [Piper TTS](https://github.com/rhasspy/piper) mit der Stimme
  `de_DE-thorsten-high` (oder einer anderen, konfigurierbaren)
- Ein lokales Wake-Word-Kommando, das dem in [CLI-Adapter-Verträge](#cli-adapter-verträge)
  beschriebenen stdout-Protokoll folgt (z. B. ein kleines Skript um
  openWakeWord oder Porcupine)
- Der `openclaw`-CLI-Adapter (siehe unten)

## Mikrofonberechtigung unter macOS

macOS verlangt für Prozesse, die auf das Mikrofon zugreifen, eine explizite
Freigabe:

1. Beim ersten Start fragt macOS automatisch nach der Mikrofonberechtigung
   für das ausführende Terminal/die Binary.
2. Falls das nicht automatisch erscheint oder abgelehnt wurde: **Systemeinstellungen
   → Datenschutz & Sicherheit → Mikrofon** öffnen und das Terminal (bzw.
   die kompilierte `openclaw-voicebridge`-Binary) aktivieren.
3. Ohne diese Freigabe liefert `openclaw-voicebridge` beim Start der Aufnahme
   einen klaren Fehler statt eines Absturzes und beendet den aktuellen
   Zyklus (Zustand geht zurück auf `IDLE`).
4. Ist gar kein Mikrofon vorhanden, wird ebenfalls ein klarer Fehler
   gemeldet - zum Testen ohne Mikrofon den `--dry-run`-Modus verwenden.

## Installation & Build

```bash
git clone <dieses-repo>
cd openclaw-voicebridge
cargo build --release
```

Die Binary liegt danach unter `target/release/openclaw-voicebridge`.

> **Hinweis:** `cargo build --release`, `cargo clippy` (ohne Warnungen)
> und `cargo test` (66/66 Tests grün) wurden auf Linux verifiziert. Das
> eigentliche CoreAudio-/Mikrofon-Verhalten sowie whisper-cli/Piper/
> OpenClaw-Integration lassen sich nur auf einem macOS-Zielsystem mit den
> tatsächlichen Binaries testen (siehe [Dry-Run](#dry-run-ohne-mikrofon)).

## Release

Die Version in `Cargo.toml` ist die einzige Quelle; Tag, Release und
Dateiname des Archivs leiten sich daraus ab. Ein Release entsteht also so:

```toml
# Cargo.toml
version = "1.0.0"
```

Diese Änderung nach `main` bringen - fertig. Der Workflow
(`.github/workflows/build-macos.yml`) erkennt beim Push nach `main`, dass
zur Version aus `Cargo.toml` noch kein Tag existiert, baut auf macOS, legt
`v1.0.0` an und veröffentlicht die Release mit
`openclaw-voicebridge-1.0.0-macos.zip` im Anhang.

Existiert der Tag zur Version bereits, passiert nichts - kein Build, kein
Release. Der Workflow läuft dadurch bei jedem Merge nach `main` an, aber
nur die Versionsprüfung selbst, nicht der macOS-Build.

Weitere Auslöser bleiben möglich:

| Auslöser | Verhalten |
|---|---|
| Push nach `main` mit erhöhter Version | Tag + Release + ZIP |
| Push nach `main` ohne Versionsänderung | nichts |
| Von Hand gepushter Tag `v*.*.*` | Release + ZIP zu diesem Tag |
| Release über die GitHub-UI angelegt | ZIP an die Release anhängen |
| `workflow_dispatch` | nur bauen, ZIP als Build-Artifact |

Ein Tag, dessen Version nicht zu `Cargo.toml` passt, bricht den Workflow
mit einer klaren Meldung ab, statt unter falscher Nummer zu
veröffentlichen.

## Konfiguration

```bash
cp config.example.toml config.toml
```

Danach `config.toml` anpassen: Pfade zu `whisper-cli`, Whisper-Modell,
Piper, `openclaw.target_channel` (siehe unten) und ggf. das Mikrofon.

Alle Werte lassen sich zusätzlich per CLI-Flag überschreiben:

```bash
openclaw-voicebridge --config /pfad/zu/config.toml --log-level debug
```

## Start

```bash
openclaw-voicebridge --config config.toml
```

Der Dienst läuft dauerhaft im Vordergrund (Zustandsmaschine in Endlosschleife)
und beendet sich sauber bei `Ctrl+C` (SIGINT) oder `SIGTERM`. Mit `--once`
wird nur ein einzelner Zyklus ausgeführt - hilfreich zum Testen. Ein
"Zyklus" beginnt dabei immer mit dem Wake-Word, kann aber intern mehrere
Gesprächsrunden umfassen, solange der Kanal offen bleibt (siehe
[Zustandsmaschine](#zustandsmaschine)).

### Nur eine Instanz gleichzeitig

Beim Start belegt `openclaw-voicebridge` eine `flock`-Sperre auf
`openclaw-voicebridge.lock` im Systemtemp-Verzeichnis. Läuft bereits eine
Instanz, bricht der zweite Start mit einer Meldung inklusive der PID der
laufenden Instanz ab, statt still danebenzulaufen.

Parallelbetrieb ist nicht vorgesehen, entsprechend gibt es dafür **keinen
Schalter**: Der Sperrpfad ist fest und hängt bewusst auch nicht an
`general.temp_dir` - sonst ließe sich die Sperre über zwei Konfigurationen
mit unterschiedlichen Pfaden aushebeln.

Der Hintergrund stammt aus dem Feldtest: Zwei parallel laufende Bridges
starten je einen eigenen Wake-Word-Listener, beide greifen auf dasselbe
Mikrofon zu und nehmen gleichzeitig auf - im Log sichtbar als doppelte
Listener-Starts pro Zyklus. Das gilt auch für einen `--dry-run`- oder
`--once`-Testlauf neben einem laufenden Dienst: die späteren Schritte
(Piper-Wiedergabe, OpenClaw-Session) würden sich sonst ebenfalls
überlagern.

Die Sperre wird vom Kernel gehalten und beim Prozessende automatisch
freigegeben - auch bei `SIGKILL` oder Absturz. Eine übrig gebliebene
Sperrdatei blockiert deshalb nichts und muss nicht aufgeräumt werden.

### Wake-Word-Schwellwert

Der Erkennungs-Schwellwert gehört in das Wake-Word-Kommando selbst (z. B.
`--threshold` des openWakeWord-Listeners), nicht in `config.toml`. Als
Anhaltspunkt aus dem Feldtest mit `hey_jarvis`: **echte** Treffer lagen bei
Scores von 0.50-0.98, ruhiges Grundrauschen erreichte aber schon ~0.20. Ein
Schwellwert von 0.1 löste dadurch sofort ohne gesprochenes Wake-Word aus und
startete eine Aufnahme; 0.5 hat sich als brauchbarer Startwert bewährt. Wer
nachjustiert, sollte die Scores des Listeners mitloggen und den Schwellwert
deutlich über dem beobachteten Grundrauschen wählen.

## Dry-Run (ohne Mikrofon)

```bash
openclaw-voicebridge --config config.toml --dry-run --dry-run-file beispiel.wav --once
```

Im Dry-Run wird das Wake-Word sofort als erkannt simuliert und die
angegebene Beispieldatei anstelle einer Mikrofonaufnahme verwendet. Der
restliche Ablauf (ffmpeg-Normalisierung, whisper-cli, OpenClaw-CLI, Piper)
läuft **real** über die konfigurierten Kommandos - so wird der komplette
Verarbeitungspfad getestet, nur der Mikrofonteil wird ersetzt. Sind
`whisper-cli`/`piper`/`openclaw` nicht installiert, schlägt der jeweilige
Schritt mit einer klaren Fehlermeldung fehl (kein Absturz).

Ein Dry-Run-Durchlauf bleibt dabei immer einmalig, auch wenn Transkript und
OpenClaw-Antwort nicht leer sind: Anders als im Realbetrieb öffnet
`openclaw-voicebridge` den Kanal danach **nicht** für eine Folgeeingabe (siehe
[Zustandsmaschine](#zustandsmaschine)), da im Dry-Run sonst dieselbe
`--dry-run-file` erneut "aufgenommen" würde - das würde bei erkannter
Sprache sonst zu einer Endlosschleife mit identischer Eingabe führen.

## CLI-Adapter-Verträge

### Wake-Word-Kommando (`wakeword.command`)

Läuft im Vordergrund, gibt bei Erkennung eine Zeile mit
`wakeword.trigger_pattern` (Standard `WAKE`) auf stdout aus und darf sich
danach beenden.

### OpenClaw-Adapter (`openclaw.binary`)

Wird aufgerufen als:

```
<binary> <extra_args...> --channel <target_channel> --message "<transkript>"
```

Die Antwort wird von stdout gelesen. `target_channel` muss in `config.toml`
gesetzt sein - ist er leer, bricht `openclaw-voicebridge` mit Fehler ab, statt
irgendeinen Standardkanal zu befüllen. Diese CLI ist bewusst ein
eigenständiger, austauschbarer Adapter: `openclaw-voicebridge` ändert nie
selbstständig OpenClaw-Konfiguration und startet nie ein Gateway neu.

### Piper (`tts.piper_binary`)

Aufruf mit `--output_file <wav>` sowie entweder `--model <model_path>`
(falls gesetzt) oder `--voice <voice>`. Text wird über stdin übergeben.

## Bestätigungstöne

Es gibt genau zwei Töne, und beide bedeuten etwas Bestimmtes. Der
Bestätigungston ist standardmäßig der macOS-Systemsound "Glass"
(`/System/Library/Sounds/Glass.aiff`, konfigurierbar über
`sound.chime_path`), der Fehlerton "Basso" (siehe [Fehlerton](#fehlerton)).
Mit `sound.enabled = false` lässt sich beides abschalten. Ein
fehlgeschlagener Ton (z. B. `afplay` nicht gefunden) wird nur geloggt und
bricht den laufenden Zyklus nicht ab.

| Signal | Bedeutung |
|---|---|
| Glass beim Start | Das Mikrofon ist offen, sprich jetzt |
| Glass am Ende | Sprache erkannt - die Aufnahme wird an OpenClaw geschickt |
| **kein** Glass am Ende | Nichts erkannt, es wird **nichts** geschickt |
| **kein** Glass nach einer Antwort | Der Kanal ist zu, es ist wieder das Wake-Word nötig |
| Basso | Der Zyklus ist mit einem Fehler abgebrochen |

Der Ende-Ton hängt also an der Spracherkennung, nicht am bloßen Ende der
Aufnahme: Er bestätigt das **Absenden**. Bleibt er aus, weiß man ohne
Hinsehen, dass die Runde ins Leere lief.

Einen eigenen "Kanal geschlossen"-Ton gibt es bewusst **nicht** mehr. Er
klang identisch zum Absende-Ton und war deshalb eher verwirrend als
hilfreich - und er ist überflüssig: Ob der Kanal nach einer Antwort noch
offen ist, hört man daran, ob ein neuer Start-Ton kommt oder nicht.

Der Start-Ton läuft **bevor** das Mikrofon geöffnet wird, der Ende-Ton
erst nachdem es wieder geschlossen ist (siehe
[Warum der Start-Ton vor dem Mikrofon kommt](#warum-der-start-ton-vor-dem-mikrofon-kommt)).

> **Offen:** Der Fall "abgeschickt, aber OpenClaw liefert keine Antwort"
> (`[Output] skipped`) ist derzeit tonlos - der Absende-Ton kam, danach
> passiert hörbar nichts mehr.

### Warum der Start-Ton vor dem Mikrofon kommt

Nicht nur der Sauberkeit halber - die umgekehrte Reihenfolge war die
Ursache eines Halluzinations-Loops im Feldtest. Bei einem Speakerphone
(Lautsprecher und Mikrofon im selben Gerät, z. B. Anker PowerConf S330)
landete der Glass-Ton laut in der eigenen Aufnahme. Gut eine Sekunde über
dem RMS-Schwellwert reicht, um `min_speech_ms` zu überschreiten: Die
VAD meldete damit bei **jeder** Aufnahme "Sprache erkannt", auch wenn
niemand sprach. Der Whisper-Skip für stille Aufnahmen konnte deshalb nie
greifen, Whisper bekam faktisch Stille zu sehen und halluzinierte daraus
Text (Untertitel-Abspänne, "Vielen Dank."), der als Eingabe wiederum eine
Antwort erzeugte und den Kanal offen hielt.

Der Ton läuft daher vor dem Öffnen des Mikrofons, gefolgt von
`audio.mic_open_delay_ms` (Standard 200 ms) Pause für das Ausklingen von
Lautsprecher und Raum. Dieselbe Pause schützt in der Folgerunde davor,
dass das Ende einer gerade vorgelesenen Antwort in die nächste Aufnahme
blutet. Der Ende-Ton läuft entsprechend erst, nachdem das Mikrofon wieder
geschlossen ist.

Erst dadurch ist `speech_started` überhaupt aussagekräftig - und damit
auch der Absende-Ton, der genau daran hängt.

Im `transcription.log` war das Symptom gut sichtbar: Zeilen wie
`[Input] "* Ding *"` sind der eigene Bestätigungston, den Whisper als
Nicht-Sprach-Ereignis transkribiert hat.

### Fehlerton

Bricht ein Zyklus **nach** erkanntem Wake-Word mit einem Fehler ab (ffmpeg,
whisper-cli, OpenClaw-Adapter oder Piper), wird ein deutlich anders
klingender Ton abgespielt - standardmäßig der macOS-Systemsound "Basso"
(`sound.error_chime_path`). Damit ist hörbar unterscheidbar, ob ein Zyklus
normal beendet wurde oder fehlgeschlagen ist, statt dass der Dienst stumm
nach `IDLE` zurückgeht und der Fehler nur im Log steht.

Fehler **während** der Wake-Word-Erkennung selbst (z. B. Wake-Word-Kommando
nicht installiert) lösen bewusst keinen Ton aus: dort wartet niemand hörbar
auf eine Antwort, und der Neustart-Loop würde sonst im Takt von
`wakeword.restart_delay_ms` dauerhaft Fehlertöne erzeugen. Wie die
Bestätigungstöne hängt auch der Fehlerton an `sound.enabled`.

## Transcription-Log

Zusätzlich zu den strukturierten `tracing`-Logs schreibt `openclaw-voicebridge`
ein einfaches, chat-artiges Log nach `transcription_log.path` (Standard:
`transcription.log` im Arbeitsverzeichnis, Append-Modus). Pro Runde gibt es
genau eine `[Input]`- und eine `[Output]`-Zeile:

```
[Input] "Hallo"
[Output] "Auch Hallo"
[Input] skipped
[Output] skipped
[Input] ignored: Untertitelung des ZDF, 2020
[Output] skipped
[Input] "Wie spät ist es?"
[Output] error
```

Text in Anführungszeichen ist immer ein **erfolgreich** übermitteltes
Transkript bzw. eine erfolgreich vorgelesene Antwort. Alles ohne
Anführungszeichen ist eine Statusmeldung für einen nicht erfolgreichen
Schritt:

- `[Input] skipped` - leeres Transkript, OpenClaw wurde nicht aufgerufen.
- `[Input] ignored: <text>` - das Transkript wurde vom
  [Transkript-Filter](#transkript-filter) als Störgeräusch/Halluzination
  verworfen. Der verworfene Text steht bewusst **ohne** Anführungszeichen
  dahinter, damit nachvollziehbar bleibt, was gefiltert wurde, ohne dass es
  wie eine übermittelte Eingabe aussieht.
- `[Output] skipped` - OpenClaw hat (bei nicht-leerem Transkript) keine
  Antwort geliefert, es wurde nichts vorgelesen.
- `[Output] error` - der OpenClaw-Aufruf oder die Piper-Wiedergabe ist
  fehlgeschlagen (der Zyklus bricht in diesem Fall danach regulär ab und
  spielt den [Fehlerton](#fehlerton) ab).

## Transkript-Filter

Die VAD erkennt Fremdgeräusche mit genug Energie (z. B. TV-Ton) als Sprache,
und Whisper macht daraus gerne typische Abspann-Halluzinationen wie
`Untertitelung des ZDF, 2020`. Ohne Gegenmaßnahme geht so etwas als
vollwertige Eingabe an OpenClaw - und hält, weil es eine Antwort erzeugt,
den Kanal für weitere Folgeeingaben offen.

`transcript_filter.ignored_patterns` enthält deshalb eine Liste von Mustern
(Default: die bekanntesten deutschen Whisper-Untertitel-Halluzinationen).
Enthält ein Transkript eines davon, wird es wie "keine Sprache" behandelt:
kein OpenClaw-Aufruf, `[Input] ignored: ...` im Transcription-Log, Kanal wird
geschlossen. Verglichen wird normalisiert (Kleinschreibung, Satzzeichen und
Apostrophe ignoriert) per Teilstring-Suche - `Untertitelung des ZDF` greift
also auch bei `Untertitelung des ZDF, 2020`. Eine leere Liste schaltet den
Filter ab.

Das ist bewusst nur das letzte Netz, nicht die tragende Schicht. Davor
liegen zwei Stufen, die verhindern sollen, dass Whisper überhaupt Stille zu
sehen bekommt: der Start-Ton außerhalb der Aufnahme (siehe
[oben](#warum-der-start-ton-vor-dem-mikrofon-kommt)) und die
Sprach-Erkennung der VAD, die `min_speech_ms` **zusammenhängend** verlangt
(`vad.speech_gap_ms`) - ohne beides öffnete sich das Gate bei jeder
Aufnahme. Was danach noch durchkommt, ist echtes Fremdgeräusch über dem
RMS-Schwellwert; der zugehörige offene Punkt (fixer
`vad.silence_rms_threshold` vs. dynamische Raumlautstärke) steht in
`ideas.md`.

Mit `transcription_log.enabled = false` lässt sich das Log abschalten. Ein
fehlgeschlagenes Schreiben (z. B. Pfad nicht beschreibbar) wird nur
geloggt und bricht den laufenden Zyklus nicht ab.

## Performance-Hinweise

Der Audio-Callback (CoreAudio-Echtzeit-Thread) verwendet bewusst
`try_send` statt `blocking_send`, um den Thread niemals zu blockieren -
das würde sonst Dropouts/Knacken im Mikrofonsignal riskieren. Bei
Backpressure (Channel voll) werden einzelne Chunks verworfen und am Ende
der Aufnahme als `dropped_chunks` geloggt; der Channel-Puffer ist mit
512 Chunks großzügig bemessen, damit das im Normalbetrieb praktisch nie
vorkommt. Der Sample-Puffer einer Aufnahme wird vorab auf eine typische
Äußerungslänge dimensioniert, damit im Normalfall keine wiederholten
Reallocations mit vollständiger Kopie anfallen - bewusst nicht auf
`max_recording_seconds`, das inzwischen ein hoch angesetztes
Sicherheitsnetz ist; längere Aufnahmen lässt der Puffer regulär wachsen.
Die
VAD-Verarbeitung berechnet die RMS-Energie direkt auf dem bestehenden
Puffer-Slice statt pro Frame einen neuen Vec zu allozieren.

## Sicherheit & Robustheit

- Keine Audiodateien werden dauerhaft gespeichert: Roh- und normalisierte
  Aufnahme werden direkt nach ihrer Verwendung gelöscht (nicht erst am
  Zyklusende), Whisper- und Piper-Zwischendateien direkt nach Gebrauch.
  Jeder Zyklus arbeitet zusätzlich in einem eigenen temporären
  Verzeichnis, das am Ende des Zyklus (auch bei Fehlern) als
  Sicherheitsnetz komplett gelöscht wird.
- Ausnahme: das [Transcription-Log](#transcription-log) speichert
  Transkript- und Antworttext bewusst dauerhaft (Append-Modus) als
  Diagnose-Hilfe - mit `transcription_log.enabled = false` abschaltbar.
- Keine API-Keys im Quellcode - alle externen Aufrufe laufen über lokale
  CLI-Kommandos, die du selbst konfigurierst.
- Timeouts für Aufnahme (`vad.silence_timeout_ms`, plus
  `max_recording_seconds` als Sicherheitsnetz), Whisper, OpenClaw und
  Piper (jeweils `timeout_secs`).
- SIGINT/SIGTERM werden abgefangen; der Dienst beendet den aktuellen Zyklus
  und stoppt danach sauber.
- Nur eine Instanz gleichzeitig (siehe
  [Nur eine Instanz gleichzeitig](#nur-eine-instanz-gleichzeitig)), damit sich
  nicht zwei Prozesse um Mikrofon und Wake-Word-Listener streiten.
- Der offene Folgeeingabe-Kanal ist auf `conversation.max_followup_turns`
  Runden begrenzt und filtert bekannte Whisper-Halluzinationen aus
  Hintergrundton heraus (siehe [Transkript-Filter](#transkript-filter)).
- Strukturierte Logs (via `tracing`) mit Zeitstempel und jedem
  Zustandswechsel.

## Tests

```bash
cargo test
```

Abgedeckt sind u. a.:

- Zustandsmaschine: erlaubte/verbotene Übergänge, vollständiger Zyklus,
  Recovery nach `IDLE` aus jedem Zwischenzustand.
- VAD-Timeout-Logik: Fortsetzen bei Sprache, Stopp nach Stille-Timeout -
  mit und ohne jemals erkannte Sprache, über denselben Weg und nach
  derselben Zeit -, Sprechen verzögert das Ende nur, Stopp beim
  Sicherheitsnetz, und `max_recording_seconds = 0` schaltet dieses ab.
- VAD-Sprach-Erkennung: verstreute laute Einzelframes öffnen das Gate
  **nicht** (Regression aus dem Feldtest), kurze Silbenpausen setzen einen
  laufenden Sprach-Abschnitt nicht zurück, eine lange Pause schon, und der
  Stille-Timeout greift auch nach einem zurückgesetzten Abschnitt.
- CLI-Argument-Konstruktion für `whisper-cli`, `ffmpeg`, den
  OpenClaw-Adapter und `piper` (inkl. `extra_args`, explizitem
  Zielkanal, Modell- vs. Stimmen-Auswahl bei Piper).
- Config-Defaults und -Validierung (u. a. Ablehnung eines leeren
  `openclaw.target_channel`), Ableitung des Sperrdatei-Pfads.
- CLI-Flag-Parsing (`--dry-run`, `--dry-run-file`, Defaults).
- Einzelinstanz-Sperre: Belegen, Ablehnen einer zweiten Sperre inkl. PID in
  der Meldung, erneutes Belegen nach Freigabe.
- Transkript-Filter: Normalisierung, Treffer auf den Halluzinationen aus dem
  Feldtest, keine Treffer auf echten Eingaben, leere Muster/Transkripte.

## Bekannte Einschränkungen / Annahmen

- Der Wake-Word-Adapter ist bewusst generisch über ein CLI-Protokoll
  gehalten statt eine konkrete Engine (Porcupine, openWakeWord, ...) fest
  einzubauen - dadurch bleibt die Wahl der Engine, inkl. Lizenzfragen, bei
  dir.
- Die Audioaufnahme unterstützt aktuell die Sample-Formate F32 und I16 des
  Standard-Eingabegeräts; andere Formate führen zu einem klaren Fehler
  statt stillem Fehlverhalten.
- Build/Tests wurden bisher nur auf Linux verifiziert. Vor Produktivbetrieb
  auf dem tatsächlichen macOS-Zielsystem `cargo build --release` und
  `cargo test` erneut ausführen sowie den `--dry-run`-Modus mit den echten
  whisper-cli/Piper/OpenClaw-Binaries durchspielen.
