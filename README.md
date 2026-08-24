# claw-voice-bridge

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
```

Jeder Zwischenzustand kann bei Fehlern/Timeouts direkt zurück nach `IDLE`
springen, damit der Dienst nicht hängen bleibt. Während `SPEAKING` läuft
keine Wake-Word-Erkennung (sie wird nur im Zustand
`LISTENING_FOR_WAKEWORD` gestartet) - die eigene TTS-Ausgabe kann sich
also nicht selbst erneut triggern.

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
   die kompilierte `claw-voice-bridge`-Binary) aktivieren.
3. Ohne diese Freigabe liefert `claw-voice-bridge` beim Start der Aufnahme
   einen klaren Fehler statt eines Absturzes und beendet den aktuellen
   Zyklus (Zustand geht zurück auf `IDLE`).
4. Ist gar kein Mikrofon vorhanden, wird ebenfalls ein klarer Fehler
   gemeldet - zum Testen ohne Mikrofon den `--dry-run`-Modus verwenden.

## Installation & Build

```bash
git clone <dieses-repo>
cd claw-voice-bridge
cargo build --release
```

Die Binary liegt danach unter `target/release/claw-voice-bridge`.

> **Hinweis:** `cargo build --release`, `cargo clippy` (ohne Warnungen)
> und `cargo test` (26/26 Tests grün) wurden auf Linux verifiziert. Das
> eigentliche CoreAudio-/Mikrofon-Verhalten sowie whisper-cli/Piper/
> OpenClaw-Integration lassen sich nur auf einem macOS-Zielsystem mit den
> tatsächlichen Binaries testen (siehe [Dry-Run](#dry-run-ohne-mikrofon)).

## Konfiguration

```bash
cp config.example.toml config.toml
```

Danach `config.toml` anpassen: Pfade zu `whisper-cli`, Whisper-Modell,
Piper, `openclaw.target_channel` (siehe unten) und ggf. das Mikrofon.

Alle Werte lassen sich zusätzlich per CLI-Flag überschreiben:

```bash
claw-voice-bridge --config /pfad/zu/config.toml --log-level debug
```

## Start

```bash
claw-voice-bridge --config config.toml
```

Der Dienst läuft dauerhaft im Vordergrund (Zustandsmaschine in Endlosschleife)
und beendet sich sauber bei `Ctrl+C` (SIGINT) oder `SIGTERM`. Mit `--once`
wird nur ein einzelner Zyklus ausgeführt - hilfreich zum Testen.

## Dry-Run (ohne Mikrofon)

```bash
claw-voice-bridge --config config.toml --dry-run --dry-run-file beispiel.wav --once
```

Im Dry-Run wird das Wake-Word sofort als erkannt simuliert und die
angegebene Beispieldatei anstelle einer Mikrofonaufnahme verwendet. Der
restliche Ablauf (ffmpeg-Normalisierung, whisper-cli, OpenClaw-CLI, Piper)
läuft **real** über die konfigurierten Kommandos - so wird der komplette
Verarbeitungspfad getestet, nur der Mikrofonteil wird ersetzt. Sind
`whisper-cli`/`piper`/`openclaw` nicht installiert, schlägt der jeweilige
Schritt mit einer klaren Fehlermeldung fehl (kein Absturz).

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
gesetzt sein - ist er leer, bricht `claw-voice-bridge` mit Fehler ab, statt
irgendeinen Standardkanal zu befüllen. Diese CLI ist bewusst ein
eigenständiger, austauschbarer Adapter: `claw-voice-bridge` ändert nie
selbstständig OpenClaw-Konfiguration und startet nie ein Gateway neu.

### Piper (`tts.piper_binary`)

Aufruf mit `--output_file <wav>` sowie entweder `--model <model_path>`
(falls gesetzt) oder `--voice <voice>`. Text wird über stdin übergeben.

## Performance-Hinweise

Der Audio-Callback (CoreAudio-Echtzeit-Thread) verwendet bewusst
`try_send` statt `blocking_send`, um den Thread niemals zu blockieren -
das würde sonst Dropouts/Knacken im Mikrofonsignal riskieren. Bei
Backpressure (Channel voll) werden einzelne Chunks verworfen und am Ende
der Aufnahme als `dropped_chunks` geloggt; der Channel-Puffer ist mit
512 Chunks großzügig bemessen, damit das im Normalbetrieb praktisch nie
vorkommt. Der Sample-Puffer einer Aufnahme wird vorab auf
`max_recording_seconds` dimensioniert, damit während der Aufnahme keine
wiederholten Reallocations mit vollständiger Kopie anfallen. Die
VAD-Verarbeitung berechnet die RMS-Energie direkt auf dem bestehenden
Puffer-Slice statt pro Frame einen neuen Vec zu allozieren.

## Sicherheit & Robustheit

- Keine Audio- oder Transkriptdateien werden dauerhaft gespeichert: jeder
  Zyklus arbeitet in einem eigenen temporären Verzeichnis, das am Ende des
  Zyklus (auch bei Fehlern) gelöscht wird.
- Keine API-Keys im Quellcode - alle externen Aufrufe laufen über lokale
  CLI-Kommandos, die du selbst konfigurierst.
- Timeouts für Aufnahme (`max_recording_seconds`), Whisper, OpenClaw und
  Piper (jeweils `timeout_secs`).
- SIGINT/SIGTERM werden abgefangen; der Dienst beendet den aktuellen Zyklus
  und stoppt danach sauber.
- Strukturierte Logs (via `tracing`) mit Zeitstempel und jedem
  Zustandswechsel.

## Tests

```bash
cargo test
```

Abgedeckt sind u. a.:

- Zustandsmaschine: erlaubte/verbotene Übergänge, vollständiger Zyklus,
  Recovery nach `IDLE` aus jedem Zwischenzustand.
- VAD-Timeout-Logik: Fortsetzen bei Sprache, Stopp nach Stille-Timeout,
  Stopp bei `max_recording_seconds`, kein Stopp vor erkannter Sprache.
- CLI-Argument-Konstruktion für `whisper-cli`, `ffmpeg`, den
  OpenClaw-Adapter und `piper` (inkl. `extra_args`, explizitem
  Zielkanal, Modell- vs. Stimmen-Auswahl bei Piper).
- Config-Defaults und -Validierung (u. a. Ablehnung eines leeren
  `openclaw.target_channel`).
- CLI-Flag-Parsing (`--dry-run`, `--dry-run-file`, Defaults).

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
