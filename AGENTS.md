# AGENTS.md

Orientierung für alle, die **an** diesem Projekt arbeiten - Menschen wie
Agenten. Die [README](README.md) beschreibt, wie man den Dienst *benutzt*;
hier steht, was man wissen muss, bevor man ihn *ändert*.

## In einem Absatz

`openclaw-voicebridge` ist ein lokaler macOS-Sprachdienst: Wake-Word →
Mikrofon → VAD → whisper.cpp → OpenClaw-CLI → Piper-TTS → Lautsprecher.
Alles läuft über lokale Prozesse, die per `config.toml` konfiguriert
werden. Der Rust-Teil kümmert sich um Audio, Zeit und Zustand; *was* die
externen Werkzeuge können und wie sie aufgerufen werden, weiß er
absichtlich nicht.

## Arbeiten am Code

```bash
cargo test              # alles, inkl. Pipeline-Test gegen Stubs
cargo clippy --all-targets
cargo fmt
cargo build --release
```

Alle drei müssen sauber durchlaufen, bevor etwas gepusht wird - clippy
ohne eine einzige Warnung.

Unter Linux braucht `cpal` die ALSA-Header, sonst bricht schon der Build ab:

```bash
apt-get install -y libasound2-dev
```

**Sprache:** Code-Kommentare, Doku und Konfigurationsbeispiele auf Deutsch;
Commit-Nachrichten und PR-Titel auf Englisch. Kommentare erklären das
*Warum*, nicht das *Was* - besonders dort, wo eine naheliegende Variante
absichtlich verworfen wurde.

## Was sich hier nicht prüfen lässt - und was trotzdem

Die Zielplattform ist macOS auf Apple Silicon. CoreAudio-Verhalten,
Mikrofonrechte, das echte whisper.cpp, Piper und OpenClaw lassen sich nur
dort testen. Entwickelt und geprüft wird trotzdem meist unter Linux.

Damit das kein blinder Fleck bleibt, gibt es zwei Integrationstests:

| Test | Braucht | Läuft bei `cargo test` |
|---|---|---|
| `tests/pipeline_with_stubs.rs` | nur `/bin/sh` | **ja** |
| `tests/dry_run.rs` | echtes ffmpeg, whisper-cli + Modell, piper, openclaw | nein (`#[ignore]`) |

`pipeline_with_stubs` legt Stub-Programme in einem Temp-Verzeichnis an, die
sich wie die echten verhalten und **mit Exit-Code 3 abbrechen, wenn die
Aufrufform nicht stimmt** - Piper muss als venv-Python mit `-m piper`
zuerst aufgerufen werden, OpenClaw mit Subkommando `agent` und dem
richtigen `--session-key`. Dann läuft eine vollständige Runde durch:
Transkript → Umschlag → JSON-Antwort → Sprachausgabe.

**Das ist das Werkzeug der Wahl für eine Änderung an der Prozesskette.**
Wer eine solche Änderung macht, sollte den Test einmal absichtlich brechen
und sehen, dass er rot wird - ein Test, der immer grün ist, prüft nichts.

Für alles Weitere: `--dry-run --dry-run-file <wav> --once` ersetzt nur die
Mikrofonaufnahme, der Rest läuft real.

## Module

| Datei | Verantwortung |
|---|---|
| `main.rs` | Zustandsmaschine, Zyklus- und Rundenablauf, Aufnahme |
| `state.rs` | erlaubte Zustandsübergänge |
| `audio.rs` | CoreAudio-Aufnahme, WAV schreiben |
| `vad.rs` | Stille-/Sprach-Erkennung (reine Logik, gut testbar) |
| `transcribe.rs` | ffmpeg-Normalisierung, whisper-cli |
| `transcript_filter.rs` | Halluzinationsfilter (letztes Netz) |
| `openclaw.rs` | Argumente, Umschlag, Antwort-Extraktion |
| `tts.rs` | Piper-Aufruf, Wiedergabe |
| `template.rs` | Platzhalter-Ersetzung in Argumentlisten (ein Durchlauf, nicht verkettet) |
| `sound.rs` | Bestätigungs- und Fehlerton |
| `instance_lock.rs` | Einzelinstanz-Sperre |
| `child_process.rs` | Prozessgruppen-Guard gegen verwaiste Enkelprozesse |
| `transcript_log.rs` | chat-artiges Diagnose-Log |
| `config.rs` | Konfiguration und Startvalidierung |

Eine Gesprächsrunde ist genau **eine** Funktion: `run_round` in `main.rs`.
Wake-Word und Folgerunde unterscheiden sich nur darin, wer sie aufruft.

## Invarianten

Diese Regeln stammen aus Fehlern, die im Feldtest wirklich passiert sind.
Wer eine davon aufweicht, holt den jeweiligen Bug zurück.

**Der Start-Ton läuft, bevor das Mikrofon aufgeht.** Bei einem Speakerphone
landete er sonst in der eigenen Aufnahme, war lauter und länger als
`min_speech_ms` - und setzte damit bei *jeder* Aufnahme "Sprache erkannt".
Der Whisper-Skip für stille Aufnahmen konnte nie greifen, Whisper
halluzinierte aus Stille Text, der als Eingabe den Kanal offen hielt. Der
Ende-Ton läuft entsprechend erst nach dem Schließen.

**Es gibt genau eine Uhr.** `silence_timeout_ms` läuft ab dem ersten Frame,
Sprechen setzt sie nur zurück. "Niemand hat gesprochen" und "jemand hat
aufgehört" enden über denselben Weg. Kein zweiter Ausgang, kein
`speech_started`-Vorbehalt. `max_recording_seconds` ist **keine**
Sprachlängenbegrenzung, sondern nur ein Sicherheitsnetz gegen unbegrenztes
Puffer-Wachstum bei Dauergeräusch.

**`min_speech_ms` meint zusammenhängende Sprache.** Der Zähler wird nach
`speech_gap_ms` Stille zurückgesetzt. Ohne das addieren sich verstreute
laute Frames über die ganze Aufnahme zu "Sprache".

**Nur eine Instanz.** Der `flock`-Pfad ist bewusst nicht konfigurierbar und
hängt nicht an `general.temp_dir` - jede Konfigurierbarkeit wäre ein Weg,
die Sperre mit zwei Konfigurationen auszuhebeln.

**Jeder Kindprozess läuft in seiner eigenen Prozessgruppe.**
`child_process::spawn_isolated` statt `cmd.spawn()` direkt. `kill_on_drop`
killt beim Drop nur die direkte Kind-PID; startet der Prozess selbst
weitere Prozesse (denkbar beim OpenClaw-CLI), liefen die sonst bei Timeout
oder Shutdown-Abbruch verwaist weiter. Der `ProcessGroupGuard` killt beim
Drop die ganze Gruppe - nach regulärem Prozessende ein wirkungsloser
No-Op.

**Der Rauschboden wird bei jedem Frame nachgeführt, nie nur bei "Stille".**
Läge die Umgebungslautstärke schon zu Beginn über der Anfangsschwelle,
würde ein Update ausschließlich bei "Stille" eingestuften Frames nie
stattfinden - die Schwelle könnte dann nie nachziehen (Deadlock). Die
asymmetrische Rate (`noise_floor_rise_alpha` klein, `noise_floor_fall_alpha`
groß) verhindert stattdessen, dass eine normale Äußerung den Boden selbst
anhebt, während dauerhaftes Geräusch (Fernseher) über viele Sekunden zur
neuen Basis wird.

**Wake-Word-Lauschen und TTS-Wiedergabe hängen an `require()`, nicht nur
an der Aufrufreihenfolge.** `sm.require(State::ListeningForWakeword)` bzw.
`sm.require(State::Speaking)` direkt vor dem jeweiligen Aufruf. Ohne diese
Prüfung wäre der richtige Zustand nur Konvention (die passende
`transition()` steht zwar davor, aber nichts erzwingt, dass sie stehen
bleibt) - ein Refactor-Fehler könnte sonst unbemerkt die eigene
TTS-Ausgabe wieder als Wake-Word einlesen lassen.

**Der Zielkanal muss das CLI erreichen.** `{channel}` ist in
`openclaw.args` Pflicht. Vorher setzte ein Adapter-Skript die Session fest
und `target_channel` steuerte nichts - die Validierung bewachte einen Wert
ohne Wirkung.

**Ein unbekanntes Config-Feld ist ein Abbruch, kein Default.** Jede
Config-Struct trägt `deny_unknown_fields`. Ohne das fiel ein Tippfehler oder
ein alter Feldname (z. B. `piper_binary` statt `binary`) beim Laden
stillschweigend auf den Default zurück - ein Bug, der nur beim ersten
Sprechversuch auffiel, nicht beim Start.

**Platzhalter werden in einem Durchlauf ersetzt, nie verkettet.**
`template::substitute` scannt `openclaw.args`/`tts.args` einmal von links
nach rechts. Zwei nacheinander ausgeführte `.replace()`-Aufrufe können einen
bereits eingesetzten Wert (z. B. `target_channel`) erneut als Platzhalter
interpretieren, wenn er zufällig dessen literalen Text enthält.

**Der Transkript-Filter ist das letzte Netz, nicht die tragende Schicht.**
Die Musterliste nicht ausbauen: Sie fängt bekannte Whisper-Halluzinationen
ab, aber die Ursache gehört davor behoben. `Vielen Dank.` gehört *nicht*
hinein - das ist eine legitime Eingabe.

**Keine Audiodateien bleiben liegen.** Roh- und normalisierte Aufnahme
werden direkt nach Gebrauch gelöscht, nicht erst am Zyklusende.

**Töne bedeuten etwas.** Glass beim Start = "sprich"; Glass am Ende =
"erkannt, wird gesendet"; kein Ton am Ende = "nichts erkannt, nichts
gesendet"; Basso = Fehler - **auch** dann, wenn zwar erfolgreich etwas
abgeschickt wurde, aber keine Antwort zurückkam (`[Output] skipped` nach
gesendetem Transkript, unterschieden von `[Output] skipped` ohne
Sendeversuch über `sent_to_openclaw` in `run_round`). Keine weiteren,
gleich klingenden Töne hinzufügen - das Ausbleiben eines Tons ist selbst
ein Signal.

**Steuernachrichten an OpenClaw (z. B. der Session-Reset) laufen NICHT
durch `message_template`.** Der Umschlag ist für Transkript-Formregeln
gedacht ("gut vorlesbare Sätze, keine Emojis") - eine Steuernachricht wie
`/new` darin einzupacken würde beim CLI nie als das erkannte Kommando
ankommen, sondern als dessen Text verpackt in der Formregel.
`openclaw::send_raw_to_openclaw` setzt `{message}` deshalb direkt, ohne
`render_message`.

## Konfiguration

**Alles, was ohne Neukompilierung änderbar ist und je nach Setup andere
Werte hat, gehört in die `config.toml`.** Das gilt ausdrücklich auch für
Argumentlisten und Prompt-Texte, nicht nur für Pfade.

Deshalb enthalten `tts.args` und `openclaw.args` die *vollständige*
Argumentliste mit Platzhaltern statt fester Flags plus `extra_args`: Wie
Piper oder OpenClaw aufgerufen werden wollen, hängt von der Installation ab
(System-Binary, Python-Modul im venv, anderer Flag-Name für den Zielkanal).
Feste Reihenfolgen erzwangen früher Wrapper-Skripte, die nichts weiter
taten als Argumente umzusortieren.

Neue Platzhalter, die weggelassen etwas still Falsches bewirken würden,
gehören in `Config::validate` - lieber ein klarer Abbruch beim Start als
ein Fehler beim ersten Sprechversuch.

## Tests

Abgedeckt sind unter anderem: Zustandsübergänge und Recovery, die
VAD-Logik in beide Richtungen, der Transkript-Filter, die Einzelinstanz-
Sperre, die Argumentkonstruktion für whisper/ffmpeg/OpenClaw/Piper, die
Antwort-Extraktion aus JSON, Config-Defaults und alle Startprüfungen sowie
ein Test, der `config.example.toml` gegen die echten Structs lädt.

Zwei Konventionen:

* Ein Test, der einen Feldfehler festhält, sagt das in seinem Namen oder
  Doc-Kommentar (`reproduces_the_venv_invocation_from_the_shell_adapter`,
  `scattered_loud_frames_never_count_as_speech`).
* Ein Test, der eine Aufrufform Argument für Argument festnagelt, ist
  Absicht: Er schlägt fehl, sobald eine Konfiguration ein früheres Skript
  nicht mehr ersetzen könnte.
* Startet ein Test die echte Binary als Kindprozess (z. B.
  `tests/pipeline_with_stubs.rs`), muss der eigentliche Programmstart durch
  einen gemeinsamen `Mutex` serialisiert werden: Der Einzelinstanz-Sperrpfad
  ist fest (siehe oben), zwei parallel gestartete Testprozesse würden sich
  sonst gegenseitig die Sperre streitig machen.

## GitHub: Branches, PRs, CI, Release

Alles, was sich auf GitHub als Plattform bezieht statt auf den Rust-Code,
steht in [`.github/AGENTS.md`](.github/AGENTS.md) - Branch-Policy
(ausschließlich `dev`, PRs statt Direkt-Push nach `main`), Pull-Request-
Vorlage, die beiden CI-Workflows und wie ein Release entsteht.

## Ideen, Roadmap, Changelog

Drei Dateien, ein Punkt gehört immer nur in genau eine davon:

* **`ideas.md`** - unsortierter Rohideen-Eimer. Alles rein, ohne Anspruch
  auf Umsetzung oder Reihenfolge.
* **`ROADMAP.md`** - nur konkret geplante, noch offene Punkte, je einer
  künftigen Version zugeordnet. Aufgebaut wie ein Stack: oben steht, was
  als Nächstes drankommt, neue Punkte werden unten angehängt.
* **`CHANGELOG.md`** - nur bereits veröffentlichte Punkte.

Wird eine Idee konkret geplant, wandert sie aus `ideas.md` in `ROADMAP.md`
und wird dort gelöscht. Wird ein Roadmap-Punkt veröffentlicht, wandert er
aus `ROADMAP.md` ins `CHANGELOG.md` und wird dort gelöscht. Keine Dopplung
zwischen den drei Dateien: Für den aktuellen Stand der geplanten Arbeit
gilt `ROADMAP.md`, für unkonkrete Ideen `ideas.md` - nicht diese Datei
hier, die nur die Spielregel dafür festhält.

## Was dieses Projekt nicht tut

Es ändert nie selbstständig OpenClaw-Konfiguration, startet nie ein Gateway
neu und speichert keine Audiodateien dauerhaft. Das
[Transcription-Log](README.md#transcription-log) ist die bewusste Ausnahme
und abschaltbar.
