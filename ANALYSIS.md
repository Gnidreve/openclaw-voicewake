Ich habe den dev-Branch des Repositories analysiert – nicht nur die Rust-Dateien, sondern auch Architektur, Konfiguration, Tests, CI/CD, Release-Prozess, AGENTS.md, ROADMAP.md, ideas.md, CHANGELOG.md, Commit-Historie und die geplante OpenClaw-WebSocket-Integration.

[Repository – dev Branch](https://github.com/Gnidreve/openclaw-voicewake/tree/dev?utm_source=chatgpt.com)

Kurzfazit

Das Projekt ist architektonisch deutlich weiter, als es die Versionsnummer 0.1.8 vermuten lässt.

Meine Einschätzung:

Bereich	Bewertung

Architektur	8.5/10
Zustands-/Fehlermodell	9/10
Konfigurationsdesign	8.5/10
Teststrategie	7.5/10
CI/CD & Releases	8.5/10
Dokumentation	9/10
Aktuelle Produktreife	6.5/10
Roadmap	7/10
Langfristige Architektur	8/10


Der wichtigste Punkt:

> Ich würde jetzt nicht anfangen, die bestehende Rust-Core-Architektur grundlegend umzubauen.



Die Richtung ist grundsätzlich richtig. Der größere nächste Schritt ist die Entkopplung vom synchronen CLI-Modell hin zu einem dauerhaft laufenden Gateway-/Event-Modell. Genau dort liegt auch die wichtigste architektonische Herausforderung der Roadmap.


---

1. Was das Projekt eigentlich ist

Das Projekt ist inzwischen nicht mehr einfach ein "Voice-Wake-Skript".

Es ist faktisch eine kleine lokale Voice Runtime für OpenClaw:

┌──────────────────┐
                │   Wake Word      │
                │ external process │
                └────────┬─────────┘
                         │
                         ▼
┌──────────┐      ┌──────────────┐
│ Microphone│ ───► │ Rust Runtime │
└──────────┘      │              │
                  │ State Machine│
                  │ VAD          │
                  │ Lifecycle    │
                  └──────┬───────┘
                         │
                         ▼
                  ┌──────────────┐
                  │ Whisper.cpp  │
                  └──────┬───────┘
                         │
                         ▼
                  ┌──────────────┐
                  │  OpenClaw    │
                  │     CLI      │
                  └──────┬───────┘
                         │
                         ▼
                  ┌──────────────┐
                  │ Piper / TTS  │
                  └──────┬───────┘
                         │
                         ▼
                     Speaker

Das Entscheidende ist: Rust besitzt den Lifecycle, während externe Tools lediglich Adapter sind.

Das wird in AGENTS.md explizit so beschrieben: Rust kümmert sich um Audio, Zeit und Zustand; die Fähigkeiten und CLI-Schnittstellen der externen Programme werden bewusst nicht in Rust fest verdrahtet. 

Das ist eine gute Entscheidung.


---

2. Die Modularchitektur

Die aktuelle Aufteilung ist ziemlich sauber:

main.rs
   │
   ├── state.rs
   │
   ├── audio.rs
   ├── vad.rs
   ├── wakeword.rs
   │
   ├── transcribe.rs
   │
   ├── openclaw.rs
   │
   ├── tts.rs
   ├── sound.rs
   │
   ├── transcript_filter.rs
   ├── transcript_log.rs
   │
   ├── config.rs
   ├── template.rs
   └── instance_lock.rs

Das entspricht auch der eigenen Modulkarte in AGENTS.md. 

✔️ Besonders gut

Die Verantwortlichkeiten sind relativ klar:

audio.rs → Audioaufnahme

vad.rs → reine VAD-Logik

state.rs → erlaubte Zustandsübergänge

openclaw.rs → OpenClaw-Protokoll/CLI

tts.rs → Piper

config.rs → Konfiguration

template.rs → Platzhalter

instance_lock.rs → Prozess-Singleton

transcript_filter.rs → letzte Schutzschicht

transcript_log.rs → Diagnose


Das ist kein monolithisches Rust-Programm, obwohl main.rs relativ groß ist.


---

3. main.rs ist trotzdem der Bereich, den ich langfristig beobachten würde

main.rs hat aktuell über 2.500 GitHub-Zeilen. 

Das ist noch kein Problem.

Interessant ist aber, was dort zusammenkommt:

State Machine

Lifecycle

Shutdown

Recording

Dry Run

Cycle Management

Follow-up Conversation

Error Handling

Temp Directory Lifecycle


Der zentrale Ansatz:

run_cycle()
    ↓
run_cycle_inner()
    ↓
run_round()

ist allerdings gut.

run_round() vereinheitlicht ausdrücklich:

Wake Word → Round
Follow-up → Round

und verhindert damit, dass zwei unterschiedliche Gesprächspipelines entstehen. 

✔️ Das würde ich beibehalten.

⚠️ Aber:

Mit der geplanten WebSocket-/Queue-Architektur wird main.rs wahrscheinlich anfangen, zu viele Verantwortlichkeiten zu bekommen.

Dann würde ich eher in Richtung:

main.rs
   │
   └── Runtime
        │
        ├── VoiceSession
        ├── AudioController
        ├── ConversationController
        ├── OutputController
        └── EventRouter

gehen.

Nicht jetzt. Erst wenn 0.2.x tatsächlich beginnt.


---

4. Die State Machine ist eine der stärksten Stellen

Aktuell:

IDLE
  ↓
LISTENING_FOR_WAKEWORD
  ↓
RECORDING
  ↓
TRANSCRIBING
  ↓
SENDING_TO_OPENCLAW
  ↓
SPEAKING
  ↓
IDLE

oder:

SPEAKING
   ↓
RECORDING

für Follow-ups.

Die erlaubten Übergänge sind tatsächlich im Code formalisiert. 

Das ist wesentlich besser als einfach irgendwo:

state = Recording;

zu setzen.

Besonders gut

Jeder Fehlerpfad kann nach IDLE zurück.

Dadurch gibt es keinen Zustand:

"Jetzt hängen wir hier für immer."

Das ist bei einem dauerhaft laufenden Voice-Dienst enorm wichtig.


---

5. Die Feldtest-getriebene Architektur ist sichtbar

Das Projekt hat etwas, das bei solchen Projekten selten ist:

Die Architektur reagiert sichtbar auf reale Fehlerszenarien.

Beispiele:

Problem

Glass-Ton wurde aufgenommen.

↓

Whisper interpretierte ihn als Sprache.

↓

Whisper halluzinierte Text.

↓

OpenClaw antwortete.

↓

Follow-up-Kanal blieb offen.

↓

Loop.

Lösung

Glass
 ↓
Microphone öffnen
 ↓
Recording

statt:

Microphone öffnen
 ↓
Glass
 ↓
Recording

Zusätzlich:

mic_open_delay_ms

und zusammenhängende Speech-Erkennung.

Das ist eine echte System-Invariante, keine kosmetische Änderung. Die Ursache und Lösung sind im Changelog und AGENTS.md dokumentiert. 

Das gleiche gilt für:

Einzelinstanz-Lock

Follow-up-Limit

Whisper-Halluzinationsfilter

kill_on_drop

Ringbuffer

Shutdown-Cancellation

deny_unknown_fields


Das ist insgesamt gutes Engineering.


---

6. Audio-Architektur

Der Wechsel vom Channel zum lock-free SPSC Ringbuffer ist sinnvoll.

Im Changelog steht explizit:

> Der Echtzeit-Audio-Callback allokiert nicht mehr auf dem Heap.



Das ist für den Audio-Callback die richtige Richtung. 

Vorher:

CoreAudio callback
      ↓
mpsc::channel<Vec<f32>>

Jetzt:

CoreAudio callback
      ↓
SPSC Ringbuffer
      ↓
Recording task

✔️ Das ist eine gute Optimierung.

Noch wichtiger:

Der Callback blockiert nicht.

Das ist bei Echtzeit-Audio wesentlich wichtiger als irgendwelche Mikrooptimierungen.


---

7. VAD

Die aktuelle VAD-Logik ist überraschend bewusst gestaltet.

Es gibt:

silence_timeout_ms

max_recording_seconds

min_speech_ms

speech_gap_ms

RMS threshold


Die zentrale Idee:

silence_timeout
       ↑
       │
 speech resets timer

und nicht:

speech detected
    ↓
start timer

Das verhindert, dass eine komplett stille Aufnahme bis zum maximalen Recording-Limit läuft. 

Aktuelles Problem

Die Roadmap erkennt selbst die Schwäche:

fixed RMS threshold
        ↓
TV / Dauergeräusch
        ↓
"Speech"

Deshalb:

0.1.11 Adaptive RMS

Das halte ich für einen der wichtigsten nächsten Punkte überhaupt. 


---

8. Der Transcript-Filter ist richtig eingeordnet

Der Filter:

VAD
 ↓
Whisper
 ↓
Transcript Filter
 ↓
OpenClaw

ist ausdrücklich nur das letzte Netz.

Das ist richtig.

Ein Filter wie:

"Untertitelung des ZDF"

ist keine echte VAD-Lösung.

Die Architektur behauptet das auch nicht.

Das ist gut.

Die Roadmap möchte das Problem später bereits auf VAD-Ebene lösen. 


---

9. Konfigurationsarchitektur

Hier wurde in den letzten Commits viel richtig gemacht.

Besonders:

[openclaw]
binary = ...
target_channel = ...
args = [...]
message_template = ...

statt:

Command::new("openclaw")
    .arg("agent")
    .arg(...)

Das macht die Bridge wesentlich weniger abhängig von einer bestimmten OpenClaw-CLI-Version.

Die Platzhalter:

{channel}
{message}
{transcript}
{voice}
{output}

sind ebenfalls sinnvoll.

Und deny_unknown_fields ist für dieses Projekt wichtig, weil ein Tippfehler in einer Voice-Konfiguration sonst extrem schwer zu erkennen wäre. 

Ein kleiner Nachteil

Die Konfiguration wird dadurch relativ mächtig.

Man kann inzwischen sehr viel über:

args = [...]

modellieren.

Das ist aktuell gerechtfertigt.

Aber langfristig sollte man aufpassen, dass config.toml nicht zu einer zweiten Programmiersprache wird.


---

10. OpenClaw-Adapter

Der aktuelle Adapter ist bewusst relativ generisch:

Config
   ↓
Template substitution
   ↓
Argument list
   ↓
Process
   ↓
stdout
   ↓
JSON extraction

extract_response() akzeptiert mehrere Response-Formate:

result.payloads[0].text
reply
text
message
content
raw stdout

Das ist pragmatisch und reduziert die Abhängigkeit vom exakten CLI-Output. 

Aber hier liegt die größte technische Schuld des aktuellen Designs:

Die CLI ist request/response-orientiert.

Das bedeutet:

Voice input
   ↓
OpenClaw CLI starten
   ↓
warten
   ↓
komplette Antwort
   ↓
TTS

Das verhindert:

echtes Streaming

sofortige ACKs

Zwischenmeldungen

Tool-Progress

Live-Output

Event-basierte externe Nachrichten


Und genau deshalb ist die WebSocket-Roadmap logisch.


---

11. WebSocket-Roadmap

Hier halte ich die geplante Richtung für richtig.

Geplant:

┌───────────────┐
             │ OpenClaw GW   │
             └───────┬───────┘
                     │ WebSocket
                     │
                     ▼
              ┌──────────────┐
              │ VoiceBridge  │
              └──────────────┘

statt:

VoiceBridge
    ↓
openclaw CLI
    ↓
wait
    ↓
result

Die Roadmap sieht zunächst einen read-only Streaming-Prototypen vor und erst danach die eigentliche Integration. Das ist aus meiner Sicht genau die richtige Reihenfolge. 

Besonders gut:

Die CLI soll nicht verschwinden.

transport = "cli"

oder:

transport = "websocket"

Das ist wichtig für:

Rückwärtskompatibilität

Debugging

Fallback

einfachere Installation

unabhängiges Testen



---

12. Aber: 0.2.x verändert die Architektur fundamental

Hier würde ich einen Punkt ergänzen, der in der Roadmap momentan noch nicht explizit genug sichtbar ist.

Mit WebSocket + Cron + Queue entsteht aus:

Voice → OpenClaw → Voice

ein Event-System.

Denn plötzlich können Ereignisse kommen aus:

Wake Word
Voice input
OpenClaw response
Cron
Notification
Streaming delta
Error
Timeout
Shutdown

Dann brauchst du konzeptionell:

┌──────────────┐
Voice ──────────►│              │
WebSocket ──────►│ Event Router │
Cron ───────────►│              │
                 └──────┬───────┘
                        │
                        ▼
                  Output Queue
                        │
                        ▼
                       TTS

Die geplante Prioritäts-/Sprech-Queue geht bereits in diese Richtung. 

Ich würde diese Queue aber nicht erst als 0.2.3 behandeln.

Sie ist eigentlich eine architektonische Voraussetzung für 0.2.1 + 0.2.2.


---

13. Größter Architekturpunkt: Audio Ownership

Das ist aus meiner Sicht der wichtigste Punkt für die nächsten Monate.

Aktuell:

Wakeword listener
        ↓
     Microphone
        ↓
     VoiceBridge

Der Wakeword-Listener ist ein externer Prozess.

ideas.md erkennt selbst das Problem:

> Der Neustart pro Zyklus war Quelle mehrerer Feldtest-Fehler.



und nennt einen nativen Rust-Wakeword-Listener als mögliche spätere Lösung. 

Ich würde das nicht kurzfristig umsetzen.

Aber langfristig sollte die Architektur auf folgendes Ziel hinauslaufen:

Audio Device
                      │
                      ▼
              ┌──────────────┐
              │ Audio Manager │
              └──────┬───────┘
                     │
           ┌─────────┴─────────┐
           ▼                   ▼
      Wake Detection        Recording

Also:

> Ein Prozess besitzt das Mikrofon.



Nicht:

Wakeword process → mic
Rust process     → mic

Das wird spätestens bei Android/WebSocket/Streaming relevant.


---

14. Datenschutz

Hier ist die Architektur grundsätzlich gut.

Audio:

microphone
 ↓
temporary file
 ↓
whisper
 ↓
delete

Keine dauerhafte Audiodatei.

Das ist sauber dokumentiert. 

Aber:

Das transcription.log ist etwas anderes.

Dort stehen:

[Input] "..."
[Output] "..."

also potenziell komplette private Gespräche.

Das Projekt dokumentiert zwar:

transcription_log.enabled = false

aber für ein späteres öffentliches Release würde ich das Verhalten noch stärker hervorheben.

Default enabled = true ist aus Privacy-Sicht diskutierbar.

Für ein persönliches Dev-System okay.

Für ein allgemein verteiltes Voice-Produkt würde ich eher:

enabled = false

als Default erwägen.


---

15. Security

Ein Punkt wird durch die neue WebSocket-Roadmap wichtiger.

Aktuell:

openclaw CLI

ist relativ klar.

Geplant:

ws://127.0.0.1:18789

Das ist zwar localhost, aber die Bridge wird dann einen Gateway-Handshake mit Scopes durchführen.

Die OpenClaw-Dokumentation zeigt inzwischen ebenfalls, dass Voice Wake eine Gateway-eigene Funktion mit WebSocket-Kommunikation und synchronisierten Clients ist. 

Deshalb würde ich für 0.2.x explizit festlegen:

Welche Scopes braucht VoiceBridge?

Darf sie nur chat.send?

Darf sie Sessions lesen?

Darf sie Session Events lesen?

Darf sie andere Nodes sehen?

Was passiert bei Auth-Fehler?

Was passiert bei Gateway-Restart?

Was passiert bei reconnect?

Was passiert bei stale subscriptions?


Diese Dinge gehören meiner Meinung nach in die Architektur-Dokumentation, bevor 0.2.1 fertig implementiert wird.


---

16. Testing

Aktuell gibt es:

src/
    unit tests

tests/
    dry_run.rs
    pipeline_with_stubs.rs



Das ist gut.

Besonders pipeline_with_stubs.rs ist interessant:

Rust
 ↓
Stub Whisper
 ↓
Stub OpenClaw
 ↓
Stub Piper

Damit wird die gesamte Prozesskette getestet, ohne die echten externen Komponenten zu benötigen. AGENTS.md beschreibt explizit, dass die Stubs bei falscher Invocation mit Exit-Code 3 scheitern. 

CI

PR:

cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check

und der macOS-Build hängt technisch von diesem Testjob ab. 

Das ist sehr ordentlich.


---

17. Was aktuell noch fehlt

Hier sehe ich die größte Lücke:

❌ Kein echter macOS-Integrationstest in CI

Das ist verständlich.

Aber:

Linux:
    Rust
    Stubs
    Pipeline

macOS:
    CoreAudio
    echte Mikrofonhardware
    whisper
    Piper
    OpenClaw

sind zwei unterschiedliche Welten.

Das Projekt dokumentiert diese Grenze korrekt. 

Langfristig wäre ein optionaler manueller Testjob sinnvoll:

workflow_dispatch
    ↓
macOS runner / self-hosted Mac
    ↓
dry-run
    ↓
real binaries

Nicht zwingend jeder PR.

Aber mindestens als Release-Gate.


---

18. Release-System

Das ist ziemlich gut gebaut.

Cargo.toml ist Single Source of Truth:

Cargo.toml
   ↓
version
   ↓
tag
   ↓
release
   ↓
ZIP

Der Workflow prüft außerdem:

Cargo version == Git tag version

und verhindert falsche Releases. 

Auch:

main push
   ↓
Ubuntu plan
   ↓
nur wenn nötig
   ↓
macOS build

ist sinnvoll, weil macOS Runner teurer sind.

✔️ Das würde ich unverändert lassen.


---

19. Git-/Entwicklungsprozess

Die Commit-Historie ist interessant.

In kurzer Zeit wurden mehrere konkrete Probleme behoben:

0.1.1
0.1.2
0.1.3
0.1.4
0.1.5
0.1.6
0.1.7
0.1.8

mit teilweise sehr spezifischen Regression-Fixes.

Die letzten Commits zeigen auch, dass dev tatsächlich als Entwicklungszweig genutzt wird und nicht nur ein zweiter Name für main ist. 

Aktuell gibt es 14 geschlossene und 1 offene PR, wobei PR #15 die 0.1.8-Shutdown-Änderung betrifft. 

Das ist für ein so junges Projekt eine ziemlich aktive Entwicklung.


---

20. Die Roadmap gefällt mir – mit einer Ausnahme

Das aktuelle Modell:

ideas.md
    ↓
ROADMAP.md
    ↓
CHANGELOG.md

ist sehr sauber.

Also:

Idee
 ↓
konkret geplant
 ↓
implementiert
 ↓
veröffentlicht

und niemals doppelt.

Das ist besser als die üblichen:

TODO.md
TODO2.md
issues
README TODO
comments

Die Roadmap beschreibt diesen Prozess ausdrücklich. 


---

21. Roadmap selbst: Priorisierung

Aktuell:

0.1.9  child processes
0.1.10 echo / double trigger
0.1.11 adaptive RMS
0.1.12 session reset
0.1.13 output skipped sound
0.1.14 config validation
        ↓
0.2 WebSocket
        ↓
0.3 tests
        ↓
0.4 background service



Meine Bewertung:

0.1.9 → 0.1.11: sehr sinnvoll

0.1.12: sinnvoll

0.1.13: nice-to-have

0.1.14: sinnvoll

0.2: strategisch richtig

0.3: meiner Meinung nach zu spät

Das ist der einzige Punkt, bei dem ich die Roadmap tatsächlich ändern würde.


---

22. Testing sollte nicht erst nach 0.2 kommen

Aktuell:

0.2
  WebSocket
  Unix Socket
  Queue

0.3
  Tests dafür

Ich würde es eher so machen:

0.2.0
  WebSocket protocol layer
  + unit tests

0.2.1
  WebSocket runtime
  + mock gateway test

0.2.2
  Unix socket
  + socket integration test

0.2.3
  queue
  + deterministic queue tests

0.3
  system-level integration

Denn gerade bei:

WebSocket
Queue
Concurrency

möchte man nicht erst nach Fertigstellung feststellen, dass die Architektur falsch war.


---

23. Die größte strategische Gefahr

Nicht Rust.

Nicht Whisper.

Nicht Piper.

Nicht VAD.

Die größte Gefahr ist Scope Expansion.

In ideas.md tauchen inzwischen auf:

WebSocket

Gateway transcription

Android

Android Release

nativer Wakeword Listener

weitere Voice-Funktionen


und die Roadmap geht bereits Richtung:

Voice
+ Streaming
+ Cron
+ Queue
+ Background service
+ Menu bar



Damit kann aus:

> "OpenClaw Voice Wake für meinen Mac"



sehr schnell werden:

> "komplette Voice Runtime / Companion Platform für OpenClaw"



Das ist technisch interessant, aber ein erheblicher Scope-Sprung.


---

24. Android würde ich deshalb noch nicht anfassen

Die Idee ist nachvollziehbar.

Aber:

macOS
 ↓
Rust binary
 ↓
CoreAudio

ist eine andere Welt als:

Android
 ↓
mobile app
 ↓
WebSocket
 ↓
Gateway

Android ist kein weiteres Target für dieselbe Binary.

Es ist ein eigenes Produkt-/Client-Modell.

Wenn Android kommt, würde ich es architektonisch eher so sehen:

OpenClaw Gateway
                          │
             ┌────────────┼────────────┐
             │            │            │
             ▼            ▼            ▼
       macOS Voice    Android App    other node

Die VoiceBridge wird dann zu einem Voice Client/Node und nicht mehr zum zentralen Voice-System.

Das ist ein wichtiger Unterschied.


---

25. Der eigentlich interessante Zielzustand

Ich würde das Projekt langfristig nicht als:

> openclaw-voicebridge



denken.

Sondern als:

OpenClaw Gateway
                    │
          ┌─────────┴─────────┐
          │                   │
       Events             Requests
          │                   │
          ▼                   ▼
   ┌──────────────────────────────┐
   │       Voice Runtime          │
   │                              │
   │ Wake Word                    │
   │ Audio                        │
   │ VAD                          │
   │ Conversation                 │
   │ Event Router                 │
   │ Output Queue                 │
   │ TTS                          │
   └──────────────────────────────┘
          │
          ▼
       Speaker

und:

CLI

ist dann nur noch ein Transport.

Das ist exakt die Richtung, in die deine 0.2-Roadmap bereits zeigt.


---

26. Was ich konkret ändern würde

Meine Reihenfolge wäre:

Phase A – 0.1 fertig machen

✔️ 0.1.9 child-process lifecycle
✔️ 0.1.10 echo/double trigger
✔️ 0.1.11 adaptive VAD
✔️ 0.1.12 session lifecycle
✔️ 0.1.14 validation

0.1.13 würde ich notfalls nach hinten schieben.


---

Phase B – WebSocket isoliert bauen

Noch keine große Runtime-Änderung.

Neue Schicht:

src/
    openclaw/
        cli.rs
        websocket.rs
        protocol.rs

oder ähnlich.

Ziel:

trait OpenClawTransport {
    ...
}

Dann:

CliTransport
WebSocketTransport

Das wäre meiner Meinung nach die wichtigste strukturelle Änderung.


---

27. Ich würde das Transport-Interface sehr früh definieren

Konzeptionell:

VoiceBridge
      │
      ▼
OpenClawTransport
      │
      ├── CLI
      │
      └── WebSocket

Nicht:

main.rs
   ├── if cli
   ├── else websocket
   ├── websocket special case
   ├── cli special case
   └── ...

Der Transport darf wissen:

wie sende ich
wie empfange ich
wie reconnecte ich

Aber nicht:

wann soll aufgenommen werden?
wann soll gesprochen werden?
wie viele Follow-ups?

Das bleibt Voice Runtime.


---

28. Danach Event-Modell

Für WebSocket würde ich früh ein internes Event-Modell einführen:

enum VoiceEvent {
    WakeWordDetected,
    SpeechStarted,
    SpeechEnded,
    TranscriptReady(String),

    OpenClawStarted,
    OpenClawDelta(String),
    OpenClawCompleted(String),

    OutputRequested(String),
    OutputCompleted,

    Timeout,
    Error(...),
}

Nicht zwingend exakt so.

Aber konzeptionell.

Damit kann später:

WebSocket
Cron
Voice
System notification

dieselbe Runtime bedienen.


---

29. Dann Output Queue

Erst dann:

┌─────────────┐
OpenClaw ──────►│             │
Cron ──────────►│ OutputQueue │
Voice ─────────►│             │
                └──────┬──────┘
                       │
                 priority
                       │
                       ▼
                      TTS

Damit ist dein geplanter 0.2.3-Schritt eigentlich die logische Konsequenz des Event-Modells.


---

30. Background Service erst danach

Die 0.4-Roadmap:

background task
↓
menu bar

ist sinnvoll. 

Aber ich würde die GUI wirklich komplett von der Runtime trennen.

Also:

┌──────────────┐
                │ Voice Runtime│
                │   daemon     │
                └──────┬───────┘
                       │
                local IPC / socket
                       │
                       ▼
                ┌──────────────┐
                │ Menu Bar App │
                └──────────────┘

Nicht:

MenuBarApp
   └── owns Voice Runtime

Sonst wird die GUI später zum architektonischen Gefängnis.


---

31. Eine Sache würde ich in der Roadmap ergänzen

Aktuell fehlt ein expliziter Punkt für:

Architecture stabilization / protocol contract

Vor 0.2.1.

Zum Beispiel:

0.2.0
  WebSocket protocol layer

0.2.0a
  Transport abstraction + internal event model

Denn sonst besteht die Gefahr:

CLI implementation
      ↓
WebSocket implementation
      ↓
Queue
      ↓
Refactoring
      ↓
nochmal Queue

Das wäre vermeidbare Arbeit.


---

32. Aktueller Reifegrad

Ich würde das Projekt momentan so einordnen:

❌ Noch kein "fertiges Produkt"

weil:

macOS-only

externe Runtime-Abhängigkeiten

Wakeword extern

keine echte native Installation

WebSocket noch nicht umgesetzt

kein permanenter Background Service

einige reale Audio-Probleme noch auf der Roadmap


✔️ Aber deutlich mehr als ein Prototyp

weil:

definierte Zustandsmaschine

Recovery

Prozess-Lifecycle

Timeouts

Shutdown

Instance Lock

VAD

Regression-Tests

Stub-Pipeline

Dry Run

Konfigurationsvalidierung

CI

Release Automation

dokumentierte Invarianten

versionierte Roadmap


Das ist für 0.1.8 bereits eine solide Basis.


---

33. Mein Gesamturteil

Architektur

Gut bis sehr gut.

Die grundlegenden Entscheidungen sind sinnvoll:

Rust = Runtime
External tools = adapters
Config = integration boundary
State machine = lifecycle authority

Code

Gut.

main.rs ist der einzige Bereich, bei dem ich langfristig aufpassen würde.

Planung

Gut, aber die Roadmap muss beim Übergang zu 0.2.x stärker architektonisch statt featureorientiert werden.

Testing

Gut für 0.1.x, aber WebSocket/Queue sollten Tests gleichzeitig mit der Implementierung bekommen.

Produktstrategie

Hier sehe ich das größte Risiko:

Voice Wake
   ↓
Voice Runtime
   ↓
Streaming
   ↓
Cron
   ↓
Notifications
   ↓
Android
   ↓
Menu Bar

Das kann sehr schnell ein deutlich größeres Projekt werden als ursprünglich geplant.


---

Meine empfohlene Ziel-Roadmap

Wenn ich deinen aktuellen Stand übernehmen würde, würde ich daraus ungefähr folgendes machen:

0.1.x
│
├─ Process lifecycle
├─ Echo / double trigger
├─ Adaptive VAD
├─ Session lifecycle
├─ Config hardening
│
▼
0.2.x  ← ARCHITECTURAL TRANSITION
│
├─ Transport abstraction
│    ├─ CLI
│    └─ WebSocket
│
├─ WebSocket protocol client
│
├─ Internal event model
│
├─ Streaming response handling
│
├─ Reconnect / Gateway restart handling
│
├─ Unix socket input
│
└─ Output queue
│
▼
0.3.x
│
├─ WebSocket integration tests
├─ Unix socket integration tests
├─ Queue tests
├─ Failure/reconnect tests
└─ Full runtime regression tests
│
▼
0.4.x
│
├─ Background daemon
├─ macOS launch integration
├─ Menu bar UI
└─ Runtime/UI separation
│
▼
0.5+
│
├─ Native wakeword/audio ownership
├─ Gateway transcription
├─ Android client
└─ Cross-device voice architecture

Der wichtigste architektonische Satz für die weitere Entwicklung wäre für mich:

> Die Voice Runtime sollte OpenClaw nicht mehr als CLI behandeln, sondern als Event-/Transport-System – die CLI ist lediglich eine Implementierung dieses Transports.



Das ist bereits implizit in deiner Roadmap angelegt. Ich würde diesen Gedanken jetzt explizit zum zentralen Architekturprinzip von 0.2.x machen.