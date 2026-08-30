# Changelog

Alle nennenswerten Änderungen an diesem Projekt werden hier dokumentiert.

Das Format orientiert sich an [Keep a Changelog](https://keepachangelog.com/de/1.1.0/),
die Versionierung an [Semantic Versioning](https://semver.org/lang/de/).

Die Version in `Cargo.toml` ist die einzige Quelle: Sie anzuheben und nach
`main` zu mergen erzeugt Tag, Release und ZIP.

Geplante, noch offene Arbeit steht in [`ROADMAP.md`](ROADMAP.md), nicht
hier - ein Punkt landet erst in diesem Changelog, wenn er umgesetzt und
veröffentlicht ist, und wird dann aus der Roadmap gelöscht.

## [Unreleased]

## [0.2.0] - 2026-08-30

### Hinzugefügt

- Neuer Transportweg `openclaw.transport = "websocket"` (Standard bleibt
  `"cli"`, beide dauerhaft unterstützt): spricht direkt mit dem
  OpenClaw-Gateway statt über das CLI. Neue Config-Felder `gateway_host`,
  `gateway_port` (Standard `18789`) und `gateway_token`.
- Ed25519-Geräteidentität (`device_identity.rs`), persistiert unter
  `~/.openclaw-voicebridge/device_identity.json`: Der Gateway-Connect-
  Handshake erfordert eine signierte Geräteidentität, ein gemeinsames
  `gateway_token` allein reicht nicht für nutzbare Scopes (verifiziert im
  OpenClaw-Quellcode). Bei einer neuen, noch nicht gekoppelten Identität
  zeigt die Fehlermeldung die `requestId` für die einmalige Genehmigung via
  `openclaw devices approve <requestId>` auf dem Gateway-Host.
- `gateway_client.rs`: verbindet sich, führt den signierten
  `connect`-Handshake aus, abonniert `openclaw.target_channel` über
  `sessions.messages.subscribe` und protokolliert eingehende Events
  (`chat`, `session.message`, `session.operation`, `session.tool`) - bewusst
  rein lesend, `chat.send` folgt erst in 0.2.1.
- `--probe-gateway`: Diagnose-Flag, das nur den WebSocket-Pfad ausführt,
  ohne Mikrofon/Wake-Word/Piper anzufassen - läuft bewusst vor der
  Einzelinstanz-Sperre, da es keine der von ihr geschützten Ressourcen
  anfasst.

## [0.1.14] - 2026-08-30

### Hinzugefügt

- Config-Validierung: `wakeword.trigger_pattern` darf nicht leer sein (passt
  sonst per Teilstring-Suche auf jede Ausgabezeile und löst die Wake-Word-
  Erkennung sofort aus), `vad.frame_ms` muss größer 0 sein (sonst bliebe
  `elapsed_ms` in der VAD für immer bei 0 stehen und weder
  `silence_timeout_ms` noch `max_recording_seconds` könnten je ablaufen),
  `openclaw.session_reset_message` darf nicht leer sein, solange
  `session_reset_after_secs` den Reset aktiviert hat.

### Geändert

- Die stderr-Ausgabe des Wake-Word-Kommandos wird jetzt eingesammelt und bei
  einem Fehlschlag (unerwartetes Prozessende) in die Fehlermeldung
  übernommen, statt wie bisher komplett verworfen zu werden - "Wake-Word-
  Prozess unerwartet beendet" allein sagte nichts über den Grund aus
  (fehlendes Modell, Python-Traceback, Mikrofon belegt, ...).

## [0.1.13] - 2026-08-30

### Hinzugefügt

- Fehlerton für `[Output] skipped`, wenn zuvor tatsächlich ein Transkript an
  OpenClaw abgeschickt wurde, aber keine Antwort zurückkam - vorher blieb
  dieser Fall akustisch unbemerkt. Der Fall "nichts erkannt, nichts
  abgeschickt" bleibt bewusst ohne Ton: Dort ist das Ausbleiben des
  Absende-Tons bereits das Signal.

## [0.1.12] - 2026-08-30

### Hinzugefügt

- OpenClaw-Session-Reset nach Inaktivität
  (`openclaw.session_reset_after_secs`, `openclaw.session_reset_message`):
  Ist seit der letzten an OpenClaw gesendeten Nachricht mehr als die
  konfigurierte Zeit vergangen, wird vor der nächsten Nachricht erst die
  Reset-Nachricht (Standard `/new`) verschickt, um eine neue Session zu
  erzwingen - eine nach langer Pause fortgesetzte alte Session brächte
  sonst stark veralteten Kontext mit ein. `0` schaltet den Reset ab. Ein
  Fehlschlag beim Reset selbst bricht die laufende Runde nicht ab.

## [0.1.11] - 2026-08-30

### Hinzugefügt

- Adaptiver Rauschboden für die Sprach-Erkennungsschwelle
  (`vad.noise_floor_margin`, `vad.noise_floor_rise_alpha`,
  `vad.noise_floor_fall_alpha`): Liegt die Umgebungslautstärke dauerhaft
  über `vad.silence_rms_threshold` (laufender Fernseher), passt sich die
  Schwelle über einige Sekunden an den tatsächlichen Pegel an, statt dass
  dieser permanent als Sprache gilt und der Stille-Timeout nie greift.
  `silence_rms_threshold` bleibt als feste Untergrenze erhalten. Die
  Anpassung ist bewusst asymmetrisch (langsam hoch, schnell runter), damit
  eine normale Äußerung den Boden nicht selbst anhebt.

### Geändert

- `silence_rms_threshold` ist jetzt eine Untergrenze, kein fixer
  Schwellwert mehr - bestehende Werte funktionieren unverändert weiter.

## [0.1.10] - 2026-08-30

### Hinzugefügt

- `StateMachine::require(state)`: bricht mit klarem Fehler ab, wenn der
  aktuelle Zustand nicht der erwartete ist. Wake-Word-Lauschen und
  TTS-Wiedergabe hingen bisher nur über die Aufrufreihenfolge in `main.rs`
  am richtigen Zustand - jetzt erzwingt eine tatsächliche Prüfung direkt
  vor dem jeweiligen Aufruf, dass beide nur in `LISTENING_FOR_WAKEWORD`
  bzw. `SPEAKING` laufen können.

## [0.1.9] - 2026-08-30

### Hinzugefügt

- Jeder Kindprozess (ffmpeg, whisper-cli, OpenClaw-CLI, Piper, Wiedergabe-
  und Wake-Word-Kommando) läuft jetzt in einer eigenen Prozessgruppe
  (`child_process::spawn_isolated`). `kill_on_drop` killt beim Drop nur
  die direkte Kind-PID - startet ein Prozess selbst weitere Prozesse,
  liefen die bei Timeout oder Shutdown-Abbruch bisher verwaist weiter. Ein
  neuer `ProcessGroupGuard` killt beim Drop die komplette Gruppe; nach
  regulärem Prozessende ist das ein wirkungsloser No-Op.

## [0.1.8] - 2026-08-30

### Hinzugefügt

- Shutdown (Ctrl+C/SIGTERM) unterbricht jetzt auch eine laufende Aufnahme,
  Whisper-Transkription, den OpenClaw-Aufruf und die Piper-TTS-Wiedergabe -
  vorher wirkte das Signal nur während der Wake-Word-Wartephase, jede
  andere Phase lief ungebremst bis zu ihrem eigenen `timeout_secs` weiter
  (im ungünstigsten Fall bis zu `max_recording_seconds`, standardmäßig
  300s). Ein noch laufender Kindprozess (ffmpeg, whisper-cli, OpenClaw-CLI,
  Piper) wird dabei über sein bestehendes `kill_on_drop(true)` beendet.
  Ein so abgebrochener Zyklus zählt nicht als Fehler: kein Fehlerton, kein
  `error!`-Log dafür.

## [0.1.7] - 2026-08-30

### Geändert

- Der Echtzeit-Audio-Callback allokiert nicht mehr auf dem Heap: Ein
  Lock-freier SPSC-Ringpuffer (`ringbuf`) ersetzt den bisherigen
  `mpsc::channel<Vec<f32>>`. F32-Samples werden per `push_slice` direkt aus
  dem cpal-Puffer übernommen, I16-Samples über einen festen Stack-
  Scratch-Puffer konvertiert - beides ohne neue Vec-Allokation pro
  Callback-Aufruf. Die Aufnahmeschleife wird über `tokio::sync::Notify`
  geweckt, sobald neue Samples verfügbar sind.
- `AudioCapture::dropped_chunks` heißt jetzt `dropped_samples`: Bei vollem
  Ringpuffer werden einzelne Samples verworfen, nicht mehr ganze Chunks
  wie beim alten Channel-basierten Ansatz.

## [0.1.6] - 2026-08-30

### Behoben

- Ein unbekanntes Config-Feld (Tippfehler oder ein alter Feldname wie
  `piper_binary`) wurde beim Laden stillschweigend ignoriert - das
  betroffene Feld fiel unbemerkt auf seinen Default zurück, statt den
  Start mit einer klaren Meldung abzubrechen. Jede Config-Struct trägt
  jetzt `deny_unknown_fields`.
- `build_openclaw_args`/`build_piper_args` ersetzten Platzhalter über zwei
  verkettete `.replace()`-Aufrufe: Enthielt ein bereits eingesetzter Wert
  (z. B. `target_channel`) selbst den literalen Text eines später
  ersetzten Platzhalters, wurde dieser Text fälschlich nochmal ersetzt.
  Ein neuer, einmaliger Ersetzungs-Durchlauf (`template::substitute`)
  behebt das.
- Schlug die SIGTERM-Registrierung fehl, deaktivierte das versehentlich
  auch Ctrl+C für den gesamten Prozess - beide laufen jetzt in getrennten,
  unabhängigen Tasks.

### Hinzugefügt

- `ROADMAP.md`: konkret geplante, noch offene Arbeit, versioniert und
  getrennt von den unkonkreten Ideen in `ideas.md` und dem bereits
  Veröffentlichten hier im Changelog.
- Der Release-ZIP enthält jetzt auch `CHANGELOG.md` neben README und
  `config.example.toml`.

### Migration

Kein Änderungsbedarf für eine korrekt geschriebene `config.toml`. Enthält
deine Config einen Tippfehler oder ein veraltetes Feld, das bisher
stillschweigend ignoriert wurde, bricht der Start jetzt mit einer klaren
Fehlermeldung ab, statt das Feld unbemerkt auf den Default zu setzen.

## [0.1.5] - 2026-08-29

Beide Shell-Adapter (`openclaw-adapter.sh`, `piper-adapter.sh`) werden
überflüssig - ihre Logik steht jetzt vollständig in der `config.toml`.

> **Migration erforderlich.** Reihenfolge: erst Binary tauschen, dann Config
> anpassen. Andersherum ignoriert das alte Binary die neuen Schlüssel.
> Bestehende `timeout_secs` übernehmen, **nicht** die Beispielwerte aus
> `config.example.toml`.

### Hinzugefügt

- `tts.args` und `tts.binary`: vollständige Piper-Argumentliste mit den
  Platzhaltern `{output}` (Pflicht) und `{voice}`. Damit ist auch eine
  venv-Installation ansprechbar, bei der `-m piper` vor allen anderen
  Argumenten stehen muss.
- `openclaw.args`: vollständige Argumentliste mit `{channel}` und `{message}`
  (beide Pflicht). Der Flag-Name für den Zielkanal steht damit in der Config -
  `--channel` bei einfachen Adaptern, `--session-key` beim echten CLI.
- `openclaw.message_template`: Umschlag um das Transkript mit `{transcript}`
  (Pflicht). Formregeln für die Sprachausgabe sind Inhalt und gehören in die
  Konfiguration, nicht in kompilierten Code.
- Auswertung der JSON-Antwort in Rust (`serde_json`): `result.payloads[0].text`,
  ersatzweise `reply`/`text`/`message`/`content`, sonst die Rohausgabe. Ein
  externer JSON-Parser wird nicht mehr gebraucht.
- Startvalidierung für alle vier Platzhalter - fehlt einer, bricht der Start
  mit klarer Meldung ab, statt später still das Falsche zu tun.
- `AGENTS.md`: Invarianten des Projekts mit ihrer Begründung, Modulkarte,
  Konfigurationsprinzip, Testkonventionen, offene Punkte.
- `tests/pipeline_with_stubs.rs`: spielt bei jedem `cargo test` eine
  vollständige Runde gegen selbst erzeugte Stub-Programme durch - ohne macOS,
  Mikrofon, Whisper-Modell oder OpenClaw. Die Stubs weisen eine falsche
  Aufrufform mit Exit-Code 3 zurück.

### Behoben

- `openclaw.target_channel` steuerte den Zielkanal nicht. Der Adapter verwarf
  `--channel` und hatte die Session hartkodiert; die Prüfung auf einen leeren
  Wert bewachte damit einen Wert ohne Wirkung.

### Entfernt

- `tts.piper_binary`, `tts.model_path`, `tts.extra_args` - durch die
  vollständige Argumentliste überflüssig.
- `openclaw.extra_args` - ebenso.

### Migration

```toml
[tts]
binary = "/pfad/zum/piper-venv/bin/python3"
voice = "de_DE-thorsten-high"
args = ["-m", "piper",
        "--data-dir", "/pfad/zu/piper-voices",
        "-m", "{voice}",
        "-f", "{output}"]
player_binary = "afplay"
timeout_secs = 60          # bisherigen Wert übernehmen

[openclaw]
binary = "/opt/homebrew/bin/openclaw"
target_channel = "agent:main:voice-assistant"   # VOLLER Session-Key, nicht der Kurzname
args = ["agent",
        "--model", "deepseek/deepseek-v4-flash",
        "--session-key", "{channel}",
        "--message", "{message}",
        "--thinking", "low",
        "--json"]
message_template = """
[Maschinengenerierte Eingabe aus der lokalen Voice-Bridge]

Zusatzregel für diese Antwort: Formuliere auf Deutsch in gut vorlesbaren,
kompakten Sätzen. Verwende keine Emojis, Smileys oder dekorative
Sonderzeichen. Diese Zusatzregel gilt nur für die Form der Antwort, nicht
als Inhalt der Benutzernachricht.

Transkript des Benutzers:
---
{transcript}
---
"""
timeout_secs = 180         # bisherigen Wert übernehmen
```

Danach können beide `.sh`-Adapter gelöscht werden. Der Wake-Word-Listener
bleibt unverändert eingebunden. Alte Schlüssel werden ignoriert - bleibt ein
Abschnitt unverändert, scheitert der Start sofort sichtbar.

## [0.1.4] - 2026-08-29

> **Migration erforderlich:** Das Binary heißt jetzt `openclaw-voicebridge`
> statt `claw-voice-bridge`. Startskripte, Launch Agents und Aliase anpassen.

### Geändert

- Paket und Binary heißen `openclaw-voicebridge`. Der Release-Workflow liest
  Name **und** Version aus `Cargo.toml`; in CI steht kein Produktname mehr
  fest verdrahtet. Die Sperrdatei heißt entsprechend
  `openclaw-voicebridge.lock` - eine noch laufende alte Instanz blockiert eine
  neue daher nicht, beim Umstieg zuerst die alte beenden.
- Das Release-Archiv heißt `openclaw-voicebridge-<version>-macos.zip` statt
  eines versionslosen Namens; Downloads verschiedener Releases sind damit
  unterscheidbar.
- Eine Gesprächsrunde ist genau eine Funktion (`run_round`). Wake-Word und
  Folgerunde unterscheiden sich nur noch darin, wer sie aufruft - beim
  Debuggen taugt die erste Runde als Referenz für alle weiteren.
- Der Stille-Timeout gilt ab dem ersten Frame. „Niemand hat gesprochen" endet
  über denselben Weg und nach derselben Zeit wie „jemand hat aufgehört zu
  sprechen"; Sprechen verzögert das Ende nur, indem es die Uhr zurücksetzt.
- `vad.max_recording_seconds` ist keine Längenbegrenzung für Sprache mehr,
  sondern ein Sicherheitsnetz gegen unbegrenztes Puffer-Wachstum bei
  Dauergeräusch. Standard von 60 auf 300 Sekunden angehoben, `0` schaltet es
  ab. Ein bestehender Wert von 60 kann bleiben.

### Hinzugefügt

- Release-Automatik: Version in `Cargo.toml` erhöhen und nach `main` mergen
  erzeugt Tag, Release und ZIP. Ohne Versionsänderung passiert nichts - ein
  vorgeschalteter Ubuntu-Job entscheidet das, damit nicht jeder Merge einen
  macOS-Runner startet.
- Ein Tag, dessen Version nicht zu `Cargo.toml` passt, bricht den Workflow mit
  klarer Meldung ab, statt unter falscher Nummer zu veröffentlichen.

### Behoben

- Eine Runde ohne jede Sprache lief bis `max_recording_seconds` - also eine
  volle Minute -, bevor sie endete. Jetzt endet sie nach dem regulären
  Stille-Timeout (Standard 4 Sekunden).

## [0.1.3] - 2026-08-26

Behebt die **Ursache** des Halluzinations-Loops, den 0.1.2 nur abgefangen hat.

### Behoben

- Der Start-Ton landete in der eigenen Aufnahme. Bei einem Speakerphone
  (Lautsprecher und Mikrofon im selben Gerät) lag er lauter und länger als
  `vad.min_speech_ms` an, wodurch die VAD bei **jeder** Aufnahme „Sprache
  erkannt" meldete. Der Whisper-Skip für stille Aufnahmen konnte deshalb nie
  greifen, Whisper halluzinierte aus faktischer Stille Text, und der ging als
  Eingabe zurück in den offenen Kanal. Der Ton läuft jetzt **vor** dem Öffnen
  des Mikrofons, der Ende-Ton erst nach dem Schließen.
- `vad.min_speech_ms` maß nicht zusammenhängende Sprache: Der Zähler wurde nie
  zurückgesetzt, sodass sich verstreute laute Frames über die gesamte Aufnahme
  zu „Sprache" aufaddierten. Bei 30-ms-Frames genügten zehn Frames irgendwo in
  bis zu 60 Sekunden.

### Hinzugefügt

- `audio.mic_open_delay_ms` (Standard 200): Pause zwischen Start-Ton und dem
  Öffnen des Mikrofons, für das Ausklingen von Lautsprecher und Raum. Schützt
  in der Folgerunde auch davor, dass das Ende einer vorgelesenen Antwort in
  die nächste Aufnahme blutet.
- `vad.speech_gap_ms` (Standard 200): zusammenhängende Stille, nach der ein
  laufender Sprach-Abschnitt als beendet gilt. Kurze Silbenpausen brechen ihn
  nicht ab.

### Geändert

- Die Ton-Sprache hat jetzt zwei Signale mit fester Bedeutung: Glass beim
  Start („sprich"), Glass am Ende („erkannt, wird gesendet"), Basso bei
  Fehlern. **Bleibt der Ton am Ende aus, wurde nichts erkannt und nichts
  abgeschickt** - das Ausbleiben ist selbst das Signal.

### Entfernt

- Der dritte, gleich klingende „Kanal geschlossen"-Ton. Er war redundant: Ob
  der Kanal nach einer Antwort noch offen ist, hört man daran, ob ein neuer
  Start-Ton kommt.

## [0.1.2] - 2026-08-26

Erste Reaktion auf den Feldtest-Bericht: fängt die Symptome des
Halluzinations-Loops ab. Die Ursache folgt in 0.1.3.

### Hinzugefügt

- Einzelinstanz-Sperre über `flock`. Zwei parallel laufende Bridges starteten
  je einen eigenen Wake-Word-Listener und griffen gleichzeitig auf dasselbe
  Mikrofon zu. Ein zweiter Start bricht jetzt mit Nennung der laufenden PID
  ab. Der Sperrpfad ist bewusst nicht konfigurierbar - Parallelbetrieb ist
  nicht vorgesehen.
- `conversation.max_followup_turns` (Standard 3): begrenzt den offenen Kanal
  auch dann, wenn jede Runde eine Antwort erzeugt. Ohne diese Grenze halten
  Fremdgeräusche im Raum - im Feldtest ein laufender Fernseher - den Kanal
  beliebig lange offen. `0` schaltet Folgeeingaben ab.
- `transcript_filter.ignored_patterns`: verwirft Transkripte mit den typischen
  Whisper-Abspann-Halluzinationen (`Untertitelung des ZDF, 2020` u. a.) wie
  „keine Sprache" - kein OpenClaw-Aufruf, Log-Zeile `[Input] ignored: …`.
- `sound.error_chime_path` (Standard macOS „Basso"): hörbar unterscheidbarer
  Ton, wenn ein Zyklus nach erkanntem Wake-Word abbricht. Fehler während der
  Wake-Word-Erkennung selbst bleiben stumm, sonst würde der Neustart-Loop
  dauerhaft piepen.
- Release-Build läuft auch beim Ereignis „Release veröffentlicht", nicht nur
  beim Tag-Push.

## [0.1.1] - 2026-08-26

### Hinzugefügt

- Chat-artiges Transcription-Log (`[Input] …` / `[Output] …`) zusätzlich zu den
  strukturierten Logs, abschaltbar über `transcription_log.enabled`.
- Release-Automatik: Ein Tag `v*.*.*` baut die macOS-Binary und veröffentlicht
  ein GitHub-Release mit ZIP.

### Behoben

- Whisper wurde auch dann aufgerufen, wenn die VAD nie echte Sprache erkannt
  hatte. Aus reiner Stille halluzinierte es nicht-leeren Text, wodurch die
  „leeres Transkript"-Prüfung ins Leere lief und sich die Folgerunden-Schleife
  aufschaukelte. Ohne erkannte Sprache werden ffmpeg-Normalisierung und
  whisper-cli jetzt komplett übersprungen.

### Geändert

- Roh- und normalisierte Aufnahme werden gelöscht, sobald sie ihren Zweck
  erfüllt haben - nicht erst am Zyklusende und unabhängig davon, wie der
  OpenClaw-Aufruf ausgeht.

## [0.1.0] - 2026-08-24

Erste Veröffentlichung.

### Hinzugefügt

- Lokaler macOS-Sprachdienst: Wake-Word → Mikrofon → VAD/Stille-Erkennung →
  whisper.cpp → OpenClaw-CLI → Piper-TTS → Lautsprecher. Alles über lokale
  Prozesse, keine Cloud-API.
- Zustandsmaschine mit Recovery nach `IDLE` aus jedem Zwischenzustand.
- Offener Kanal für Folgeeingaben: Nach einer vorgelesenen Antwort ist ohne
  erneutes Wake-Word eine weitere Eingabe möglich.
- Bestätigungstöne bei Aufnahme-Start und -Ende.
- `--dry-run` mit `--dry-run-file` als Ersatz für die Mikrofonaufnahme.
- Manuell auslösbarer GitHub-Actions-Workflow für den macOS-Release-Build.

### Behoben

- SIGINT/SIGTERM wurden beim Warten auf das Wake-Word nicht beachtet - dem
  häufigsten Ruhezustand -, der Dienst ließ sich dort nicht sauber beenden.
- Kindprozesse (ffmpeg, whisper-cli, OpenClaw, Piper, afplay) wurden bei
  Timeout nicht beendet, sondern verwaisten.
- `wakeword.restart_delay_ms` war definiert, wurde aber nirgends gelesen: Ein
  dauerhaft fehlschlagendes Wake-Word-Kommando lief ungebremst im Busy-Loop.

[Unreleased]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.14...v0.2.0
[0.1.14]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.13...v0.1.14
[0.1.13]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.12...v0.1.13
[0.1.12]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/Gnidreve/openclaw-voicewake/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Gnidreve/openclaw-voicewake/releases/tag/v0.1.0