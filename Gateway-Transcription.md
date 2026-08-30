# OpenClaw Gateway – External WebSocket Audio Transcription Integration

## Zweck dieses Dokuments

Dieses Dokument beschreibt die technische Recherche und die erwartete Integrationsarchitektur für eine fremde Anwendung, die Audio an ein OpenClaw Gateway senden möchte, damit OpenClaw die Sprachtranskription übernimmt.

Die Implementierung soll in der Zielanwendung erfolgen.

Der Implementierungs-Agent soll dieses Dokument als Recherche-/Diagnosegrundlage verwenden und die konkreten Details anschließend anhand der tatsächlichen Codebasis, OpenClaw-Version und vorhandenen WebSocket-Infrastruktur verifizieren.

---

# 1. Ziel

Die Zielanwendung soll Audio an ein OpenClaw Gateway senden können, ohne selbst einen STT-Provider wie OpenAI, Deepgram, Groq etc. implementieren oder direkt ansprechen zu müssen.

Gewünschter Datenfluss:

    Target Application
        |
        | WebSocket
        v
    OpenClaw Gateway
        |
        | Gateway-owned Talk transcription session
        v
    Configured STT Provider
        |
        | transcript events
        v
    OpenClaw Gateway
        |
        | talk.event
        v
    Target Application

Die Zielanwendung soll dabei ausschließlich für:

- Audioaufnahme oder Audioquelle
- Audioformat-Konvertierung
- WebSocket-Verbindung
- Gateway-Authentifizierung
- Talk-Session-Lifecycle
- Senden der Audio-Chunks
- Empfangen und Verarbeiten der Transcript-Events

verantwortlich sein.

Die Zielanwendung soll NICHT selbst:

- OpenAI STT aufrufen
- Deepgram aufrufen
- Whisper aufrufen
- STT-Provider auswählen
- Provider-Credentials verwalten
- Provider-spezifische WebSocket-Verbindungen herstellen
- Provider-spezifische Realtime-Protokolle implementieren

Diese Verantwortung verbleibt beim OpenClaw Gateway.

---

# 2. Wichtig: Nicht mit Telegram koppeln

Telegram ist für diese Integration nicht erforderlich.

Die native Telegram-Integration von OpenClaw ist nur ein Beispiel für einen Channel, der Audio an OpenClaw übergibt.

Für eine externe Anwendung kann die Telegram-Schicht vollständig entfallen.

Es gibt zwei konzeptionell unterschiedliche OpenClaw-Audio-Pfade:

## A. One-shot / Batch Audio

Typischer Ablauf:

    Audio File
        |
        v
    OpenClaw Media Understanding
        |
        v
    tools.media.audio
        |
        v
    Batch STT Provider
        |
        v
    Transcript

Dieser Pfad wird typischerweise für fertige Voice Messages / Audio Attachments verwendet.

## B. Gateway-owned Streaming Transcription

Typischer Ablauf:

    Audio Stream
        |
        v
    WebSocket
        |
        v
    talk.session.create
        mode = transcription
        transport = gateway-relay
        |
        v
    talk.session.appendAudio
        |
        v
    Gateway-owned STT session
        |
        v
    Transcript events

Dieser Pfad ist für externe Clients relevant, wenn Audio über die Gateway-WebSocket-Verbindung gestreamt werden soll.

Die aktuelle OpenClaw-Dokumentation nennt explizit:

    talk.session.create({
      mode: "transcription",
      transport: "gateway-relay",
      brain: "none"
    })

gefolgt von:

    talk.session.appendAudio

und anschließend:

    talk.session.close

für Transcription-only-Clients.

---

# 3. Relevanter OpenClaw API-Pfad

Für diese Integration ist der aktuelle Unified Talk API-Pfad relevant:

    talk.session.create
    talk.session.appendAudio
    talk.event
    talk.session.close

Nicht verwenden:

    talk.transcription.session
    talk.transcription.relayAudio
    talk.transcription.relayStop

Diese älteren API-Familien wurden durch die Unified Talk API ersetzt.

Ebenso sind ältere:

    talk.realtime.*

APIs nicht die Zielimplementierung.

Aktuelle API:

    talk.session.*

Quelle:
OpenClaw Gateway Protocol und SDK-Migrationsdokumentation.

---

# 4. Session-Modus

Für reine Transkription ist die Session-Konfiguration:

    mode: "transcription"
    transport: "gateway-relay"
    brain: "none"

Konzeptionell:

    {
      mode: "transcription",
      transport: "gateway-relay",
      brain: "none"
    }

Bedeutung:

- `transcription`
  - Die Session soll Audio transkribieren.
  - Es wird keine vollständige Realtime-Sprachinteraktion benötigt.

- `gateway-relay`
  - Das Gateway besitzt und kontrolliert die Provider-Session.
  - Der Client muss keine direkte Provider-WebSocket-Verbindung aufbauen.

- `brain: "none"`
  - Es wird kein Agent-/Realtime-Brain benötigt.
  - Der Zweck der Session ist reine Transkription.

Die aktuelle OpenClaw SDK-Migrationsdokumentation beschreibt genau diese Kombination als:

    transcription / gateway-relay / none

und bezeichnet sie als:

    Streaming STT only.

---

# 5. Grundlegender Ablauf

Der Implementierungs-Agent soll den Client nach folgendem Ablauf strukturieren:

    connect WebSocket
        |
        v
    authenticate / establish Gateway protocol session
        |
        v
    talk.session.create
        |
        v
    receive session result
        |
        v
    obtain sessionId
        |
        v
    stream PCM audio
        |
        +----> talk.session.appendAudio
        |
        +----> talk.session.appendAudio
        |
        +----> talk.session.appendAudio
        |
        v
    receive talk.event messages
        |
        v
    extract transcript events
        |
        v
    when recording/transcription is finished:
        |
        v
    talk.session.close
        |
        v
    dispose local session state

---

# 6. WebSocket ist nicht der STT-Provider

Wichtiges Architekturprinzip:

Die Zielanwendung spricht NICHT direkt mit dem STT-Provider.

Nicht:

    Client
       |
       +--> OpenClaw Gateway
       |
       +--> OpenAI
       |
       +--> Deepgram

Sondern:

    Client
       |
       | WebSocket
       v
    OpenClaw Gateway
       |
       | provider-specific connection
       v
    Configured STT Provider

Das Gateway abstrahiert:

- Provider
- Modell
- API-Key
- Provider-WebSocket
- Provider-Session
- Provider-spezifische Events
- Provider-spezifische Audioanforderungen

Die Zielanwendung kennt diese Details nicht.

---

# 7. Audioformat

Der aktuelle Gateway-owned Realtime/Transcription-Pfad erwartet PCM-Audio.

`talk.session.appendAudio` akzeptiert Base64-kodiertes PCM-Audio.

Die konkrete Audio-Konfiguration muss anhand der Response von `talk.session.create` und des verwendeten OpenClaw-Stands verifiziert werden.

Der aktuelle Realtime-Relay-Code verwendet:

    PCM16
    24 kHz

Der aktuelle Source-Code bezeichnet das Format als:

    REALTIME_VOICE_AUDIO_FORMAT_PCM16_24KHZ

und die Session-Response enthält entsprechende Informationen wie:

    inputEncoding: "pcm16"
    inputSampleRateHz: 24000

Daher sollte die Implementierung zunächst von folgendem Ziel ausgehen:

    Encoding:
        PCM16

    Sample rate:
        24000 Hz

    Channels:
        prüfen/verifizieren

    Transport:
        Base64 innerhalb der Gateway-RPC-Payload

NICHT ungeprüft direkt senden:

    OGG/Opus
    MP3
    AAC
    M4A
    WebM/Opus

Diese Formate sind für fertige Audio-Dateien geeignet, aber nicht automatisch das Eingabeformat von `talk.session.appendAudio`.

Wenn die ursprüngliche Audioquelle z. B. Telegram Voice oder WebM/Opus liefert, muss die Zielanwendung das Audio vor dem WebSocket-Streaming dekodieren bzw. resamplen.

Beispiel:

    Telegram / Browser / File
            |
            v
        OGG/Opus
            |
            v
       decode/resample
            |
            v
        PCM16 24kHz
            |
            v
       Base64 encode
            |
            v
    talk.session.appendAudio

---

# 8. `talk.session.create`

Der Client muss zunächst eine Gateway-owned Talk Session erzeugen.

Konzeptionelle Request-Struktur:

    {
      "mode": "transcription",
      "transport": "gateway-relay",
      "brain": "none"
    }

Die genaue RPC-Envelope muss anhand der vorhandenen Gateway-WebSocket-Implementierung der Zielanwendung angepasst werden.

Wichtig:

Die konkrete Request-ID, Connection-Handshake-Struktur und Authentifizierung dürfen NICHT aus diesem Dokument erfunden werden.

Der Implementierungs-Agent muss sie aus:

- vorhandener WebSocket-Client-Implementierung
- OpenClaw Gateway Protocol
- verwendeter OpenClaw-Version

ableiten.

---

# 9. Session Response

Nach erfolgreichem `talk.session.create` muss der Client die Session-ID aus der Response übernehmen.

Diese Session-ID wird anschließend für:

    talk.session.appendAudio

und:

    talk.session.close

verwendet.

Die Session-ID darf nicht global für mehrere Audioquellen wiederverwendet werden.

Empfohlene Client-Struktur:

    TalkTranscriptionSession
        sessionId
        connection
        state
        audioFormat
        startedAt
        lastAudioAt
        transcript
        partialTranscript
        closed

---

# 10. Audio senden

Audio wird chunkweise übertragen.

Konzeptionell:

    PCM audio bytes
        |
        v
    Base64
        |
        v
    talk.session.appendAudio

Payload-Konzept:

    {
      "sessionId": "...",
      "audioBase64": "..."
    }

Der genaue Property-Name und die RPC-Envelope müssen gegen das aktuelle Gateway-Schema geprüft werden.

Der aktuelle OpenClaw SDK-Code verwendet für den Gateway-Aufruf:

    gateway.request(
      "talk.session.appendAudio",
      {
        sessionId,
        audioBase64
      }
    )

Das ist die maßgebliche aktuelle API-Form.

---

# 11. Chunking

Die Zielanwendung sollte Audio nicht als riesigen Block behandeln.

Stattdessen:

    microphone
        |
        v
    PCM frames
        |
        v
    chunk buffer
        |
        v
    Base64
        |
        v
    appendAudio
        |
        v
    WebSocket

Die konkrete Chunk-Größe sollte nicht hart aus dieser Dokumentation übernommen werden.

Der Implementierungs-Agent soll prüfen:

- welche Chunk-Größe die vorhandene Audio-Pipeline erzeugt
- welche Latenz benötigt wird
- ob das Gateway intern Chunk-Limits besitzt
- ob Backpressure berücksichtigt werden muss

Wichtig:

Die Implementierung muss verhindern, dass bei langsamer WebSocket-Verbindung unbegrenzt Audio im RAM gepuffert wird.

---

# 12. Backpressure

Der Client muss den Fall behandeln:

    audio producer
          |
          v
    WebSocket sender
          |
          X
      slower than producer

Es darf nicht unbegrenzt wachsen:

    pendingAudioChunks[]

Empfehlung:

- bounded queue
- maximal zulässige Queue-Größe
- kontrolliertes Dropping oder Recording-Abbruch
- Monitoring der Sendelatenz
- Session-Abbruch bei dauerhaftem Transportfehler

Für Voice-Transkription ist ein kontrollierter Fehler besser als unbounded memory growth.

---

# 13. Transcript Events

Das Gateway verwendet einen gemeinsamen Talk-Event-Kanal:

    talk.event

Dieser Kanal wird für:

- realtime
- transcription
- STT/TTS
- managed-room
- telephony
- meeting adapters

verwendet.

Die Zielanwendung darf daher nicht davon ausgehen, dass jedes `talk.event` ausschließlich ein Transcript enthält.

Sie muss Events anhand ihres Event-Typs klassifizieren.

Konzeptionell:

    talk.event
        |
        +-- session / lifecycle
        |
        +-- turn
        |
        +-- transcript
        |
        +-- error
        |
        +-- other Talk events

Der konkrete Event-Payload muss anhand des aktuellen Gateway-Protokolls bzw. der verwendeten OpenClaw-Version verifiziert werden.

Nicht anhand von String-Matching auf beliebige Payloads implementieren.

---

# 14. Partial vs. Final Transcript

Ein Streaming-STT-System kann Zwischenresultate liefern.

Beispiel:

    "Hallo"
    "Hallo OpenClaw"
    "Hallo OpenClaw bitte"
    "Hallo OpenClaw bitte fasse"
    "Hallo OpenClaw bitte fasse das zusammen"

Diese Events dürfen nicht automatisch als fünf verschiedene Benutzer-Nachrichten interpretiert werden.

Die Client-Anwendung sollte zwischen:

    partial transcript

und:

    final transcript

unterscheiden.

Empfohlener Zustand:

    partialTranscript
    finalTranscript

Beispiel:

    partial:
        "Hallo OpenClaw bitte fasse"

    final:
        "Hallo OpenClaw bitte fasse das zusammen"

Der Client soll erst beim finalen Transcript eine abgeschlossene Voice-Input-Operation signalisieren.

---

# 15. Session Lifecycle

Empfohlener Lifecycle:

    IDLE
      |
      v
    CREATING
      |
      v
    ACTIVE
      |
      v
    STREAMING
      |
      v
    FINALIZING
      |
      v
    CLOSED

Fehler:

    CREATING -> ERROR
    ACTIVE   -> ERROR
    STREAMING -> ERROR

Nicht zulassen:

    appendAudio()
        nachdem
    session.close()

ausgeführt wurde.

Nach `close` muss eine neue Session erstellt werden.

---

# 16. Session Close

Wenn keine weiteren Audio-Daten gesendet werden sollen:

    talk.session.close

aufrufen.

Die Session darf nicht einfach nur lokal verworfen werden.

Ziel:

    stop audio
       |
       v
    stop appendAudio
       |
       v
    close Gateway Talk session
       |
       v
    receive final lifecycle events
       |
       v
    release local resources

Der konkrete Zeitpunkt des Close-Aufrufs muss mit dem erwarteten Final-Transcript-Verhalten des aktuellen OpenClaw-Stands abgeglichen werden.

Insbesondere prüfen:

- Wird das letzte Audio automatisch finalisiert?
- Ist ein expliziter Turn-End erforderlich?
- Wann kommt das finale Transcript?
- Wann darf `close` aufgerufen werden?

Bei `mode = transcription` ist die Session-Dokumentation maßgeblich.

---

# 17. `startTurn` / `endTurn`

Nicht automatisch verwenden.

Die aktuelle Unified Talk API besitzt:

    talk.session.startTurn
    talk.session.endTurn

Diese Methoden gehören insbesondere zum:

    stt-tts / managed-room

Pfad.

Für:

    transcription / gateway-relay

ist der primäre Mechanismus:

    create
    appendAudio
    close

Der Implementierungs-Agent muss anhand der aktuellen Schema-Definition prüfen, ob ein explizites Turn-End im jeweiligen OpenClaw-Stand notwendig ist.

Nicht aus dem `managed-room`-Verhalten ableiten.

---

# 18. `talk.client.create` nicht mit `talk.session.create` verwechseln

Es gibt zwei unterschiedliche Konzepte:

## Gateway-owned session

    talk.session.create

Der Gateway besitzt die Provider-Session.

Geeignet für:

    transcription / gateway-relay

und:

    realtime / gateway-relay

## Client-owned realtime session

    talk.client.create

Hier kann der Client eine Provider-seitige Realtime-Verbindung bzw. einen client-owned Transport verwenden.

Für das hier beschriebene Ziel ist das NICHT die bevorzugte API.

Ziel:

    target app
        |
        v
    OpenClaw Gateway
        |
        v
    configured STT provider

Daher:

    talk.session.create

verwenden.

---

# 19. Warum `gateway-relay` wichtig ist

`gateway-relay` bedeutet architektonisch:

    Client
       |
       | audio
       v
    OpenClaw Gateway
       |
       | provider protocol
       v
    STT Provider

Dadurch bleiben:

- API Keys
- Provider URLs
- Provider Configuration
- Model Selection
- Provider Sessions

auf dem Gateway.

Das ist genau der gewünschte Abstraktionspunkt.

---

# 20. Provider-Auswahl

Der externe Client soll NICHT auswählen:

    provider = openai

oder:

    provider = deepgram

wenn das Ziel eine Gateway-zentrale Provider-Konfiguration ist.

Stattdessen:

    Client
       |
       | transcription session
       v
    Gateway
       |
       v
    configured Talk transcription provider

Der Gateway-Administrator konfiguriert den Provider.

Der Client fragt optional:

    talk.catalog

ab, wenn er Informationen über die verfügbaren Talk-/Transcription-Fähigkeiten benötigt.

`talk.catalog` liefert laut Gateway-Protokoll unter anderem:

- Provider IDs
- Labels
- configured state
- Models
- Voices
- Modi
- Transports
- Realtime capabilities

Secrets werden dabei nicht ausgegeben.

---

# 21. Konfiguration und Discovery

Wenn die Zielanwendung dynamisch prüfen möchte, ob Streaming Transcription verfügbar ist:

    talk.catalog

verwenden.

Optional kann:

    talk.config

verwendet werden, wenn die Client-Anwendung berechtigten Zugriff auf die effektive Talk-Konfiguration benötigt.

Die Zielanwendung sollte keine Provider-Konfiguration aus der lokalen OpenClaw-Datei replizieren.

Besser:

    Gateway = source of truth

---

# 22. Fehlerbehandlung

Mindestens folgende Fehlerklassen behandeln:

## Session Creation Failure

Beispiele:

- kein geeigneter Provider
- Provider nicht konfiguriert
- Auth fehlt
- Talk nicht verfügbar
- ungültige Session-Parameter

Verhalten:

    CREATE -> ERROR

Keine Audio-Chunks senden.

---

## Unknown Session

Wenn:

    talk.session.appendAudio

mit einer unbekannten Session beantwortet wird:

Mögliche Ursachen:

- Session bereits geschlossen
- Session abgelaufen
- falsche Session-ID
- Gateway-Verbindung neu aufgebaut
- Gateway hat Session verworfen

Nicht automatisch weiter Audio senden.

Stattdessen:

    stop producer
    cleanup session
    optionally create new session

---

## Provider Failure

Der Gateway kann Provider-Fehler an die Talk-Event-/Session-Ebene weitergeben.

Die Client-Anwendung muss:

- Fehler sichtbar machen
- Recording stoppen
- Session schließen/cleanup
- keine Endlosschleife beim Resend starten

---

## WebSocket Disconnect

Bei WebSocket Disconnect:

    session state = invalid

Die alte Session-ID nicht wiederverwenden.

Nach Wiederverbindung:

    new WebSocket
        |
        v
    new Gateway session
        |
        v
    new talk.session.create

---

# 23. Authentifizierung

Dieses Dokument beschreibt NICHT die Authentifizierungsdetails des Gateway-WebSockets.

Der Implementierungs-Agent muss die vorhandene Gateway-Client-Implementierung untersuchen.

Insbesondere prüfen:

- connect handshake
- authentication challenge
- auth token
- device identity
- scopes / operator permissions
- protocol version
- connection lifecycle

Die Talk-Methoden unterliegen den normalen Gateway-Berechtigungen.

Keine Authentifizierungslogik erfinden.

---

# 24. Berechtigungen

Die Implementierung muss die tatsächlichen Gateway-Scopes der verwendeten OpenClaw-Version berücksichtigen.

Nicht automatisch davon ausgehen:

    unauthenticated client
        -> talk.session.create

ist erlaubt.

Bei Fehlern wie:

    unauthorized
    forbidden
    invalid scope

muss die Gateway-Auth-Schicht geprüft werden.

---

# 25. Audioquelle

Die Zielanwendung kann Audio aus verschiedenen Quellen erhalten:

    microphone
    uploaded file
    Telegram
    Discord
    browser MediaRecorder
    mobile recorder
    RTP source
    another WebSocket
    local file

Die Quelle ist für den Gateway nicht relevant, solange die Daten in das erwartete Audioformat gebracht werden.

Beispiel:

    Browser microphone
        |
        v
    Audio processing
        |
        v
    PCM16 / 24kHz
        |
        v
    Gateway

oder:

    Telegram voice.ogg
        |
        v
    ffmpeg / decoder
        |
        v
    PCM16 / 24kHz
        |
        v
    Gateway

---

# 26. Batch-Datei vs. Streaming

Nicht versuchen, eine fertige Voice-Datei 1:1 als einen großen `appendAudio`-Request zu verwenden.

Das Ziel des `talk.session`-Transcription-Modus ist Streaming.

Wenn eine Datei vorliegt:

    file
      |
      v
    decode
      |
      v
    PCM
      |
      v
    chunk
      |
      v
    appendAudio
      |
      v
    appendAudio
      |
      v
    ...

Dadurch verhält sich die Datei technisch wie ein Audio-Stream.

---

# 27. Telegram-spezifische Erkenntnis

Die Telegram-Integration ist nicht die technische Voraussetzung für Gateway-STT.

Telegram zeigt lediglich, wie ein Channel Audio in OpenClaw einspeist.

Die Zielanwendung darf daher NICHT versuchen, die Telegram-Implementierung zu kopieren, wenn sie direkt mit dem Gateway-WebSocket arbeitet.

Stattdessen soll sie die Gateway-Talk-API direkt verwenden.

---

# 28. Unterschied zum Media-Understanding-Pfad

Nicht verwechseln:

## Media Understanding

    tools.media.audio

Eigenschaften:

- fertige Audio-Datei
- Batch-Verarbeitung
- Media Attachment
- Transcript wird in Message Context integriert
- geeignet für Voice Notes

## Gateway Talk Transcription

    talk.session.create
        mode = transcription
        transport = gateway-relay

Eigenschaften:

- Streaming
- WebSocket
- Gateway-owned STT session
- Transcript Events
- geeignet für externe Live-Audio-Clients

Die aktuelle OpenClaw-Dokumentation beschreibt ausdrücklich:

    "One-shot uploaded voice notes still use the media understanding audio path."

Daher ist eine fertige Voice Note nicht automatisch dasselbe wie eine Talk-Transcription-Session.

---

# 29. Wenn das Ziel eine normale Agent-Nachricht ist

Wichtig:

Eine reine:

    transcription / gateway-relay

Session liefert Transkription.

Sie ist nicht automatisch gleichbedeutend mit:

    normale OpenClaw Chat Message
        ->
    Agent Run
        ->
    Agent Response

Wenn die Zielanwendung Folgendes benötigt:

    microphone
       |
       v
    STT
       |
       v
    normal OpenClaw agent message
       |
       v
    agent response

muss zusätzlich untersucht werden, wie die Zielanwendung das fertige Transcript in den normalen OpenClaw Message-/Agent-Routing-Pfad übergeben soll.

Die Transcription Session und der Agent-Message-Lifecycle sind zwei unterschiedliche Schichten.

Nicht eigenständig annehmen, dass:

    final transcript event

automatisch:

    agent invocation

bedeutet.

---

# 30. Empfohlene Architektur der Zielanwendung

Die Implementierung sollte mindestens folgende Komponenten trennen:

    OpenClawGatewayClient
        |
        +-- connect()
        +-- authenticate()
        +-- request()
        +-- subscribeToEvents()

    TalkTranscriptionSession
        |
        +-- create()
        +-- appendAudio()
        +-- close()
        +-- handleEvent()

    AudioConverter
        |
        +-- decode()
        +-- resample()
        +-- convertToPCM16()

    TranscriptAccumulator
        |
        +-- handlePartial()
        +-- handleFinal()
        +-- getCurrentTranscript()

    Optional:
    AgentMessageBridge
        |
        +-- sendTranscriptToOpenClawAgent()

Die Providerlogik gehört NICHT in diese Komponenten.

---

# 31. Zustandsmaschine

Empfohlene Zustandsmaschine:

    IDLE
      |
      | start()
      v
    CONNECTING
      |
      v
    CREATING_SESSION
      |
      | success
      v
    READY
      |
      | audio
      v
    STREAMING
      |
      | stop
      v
    FINALIZING
      |
      | final transcript / close
      v
    CLOSED

Fehler aus jedem Zustand:

    ERROR

Nach ERROR:

    cleanup
      |
      v
    IDLE

---

# 32. Diagnostik

Der Implementierungs-Agent soll beim Debugging mindestens folgende Informationen loggen:

    Gateway connection established
    Gateway protocol established
    talk.session.create requested
    talk.session.create succeeded
    sessionId
    negotiated audio format
    audio chunk count
    audio bytes sent
    appendAudio latency
    transcript event received
    transcript partial/final
    session close requested
    session closed
    provider/session error

Keine Secrets loggen.

Insbesondere NICHT:

- API keys
- auth tokens
- provider credentials
- session secrets

---

# 33. Debugging-Reihenfolge

Wenn keine Transkription funktioniert:

## Schritt 1

Prüfen:

    WebSocket connected?

## Schritt 2

Prüfen:

    Gateway protocol handshake successful?

## Schritt 3

Prüfen:

    talk.session.create successful?

## Schritt 4

Prüfen:

    sessionId vorhanden?

## Schritt 5

Prüfen:

    negotiated input format?

## Schritt 6

Prüfen:

    appendAudio requests successfully sent?

## Schritt 7

Prüfen:

    talk.event messages received?

## Schritt 8

Prüfen:

    transcript events vorhanden?

## Schritt 9

Wenn Session existiert, aber keine Transkription:

    Provider configuration
    provider availability
    audio format
    audio sample rate
    audio encoding
    session lifecycle

prüfen.

---

# 34. Audio-Diagnostik

Ein besonders häufiger Fehler ist:

    WebSocket works
    Session works
    appendAudio works
    No transcript

In diesem Fall nicht zuerst den Provider-Code ändern.

Zuerst prüfen:

    Is the audio actually valid PCM?

    Is it PCM16?

    Is the sample rate correct?

    Is it mono/stereo as expected?

    Are samples little-endian as expected?

    Is Base64 decoded correctly?

    Are chunks ordered?

    Is silence actually being sent?

    Is the audio duration > 0?

---

# 35. Keine Opus-Daten als PCM tarnen

Ein häufiger Implementierungsfehler:

    OGG/Opus bytes
        |
        v
    Base64
        |
        v
    appendAudio

Das ist falsch, wenn die Session PCM erwartet.

Base64 ist nur die Transportkodierung.

Es konvertiert:

    Opus -> PCM

NICHT.

Korrekt:

    Opus
      |
      v
    decoder
      |
      v
    PCM16
      |
      v
    Base64
      |
      v
    appendAudio

---

# 36. Versionierung

Diese Spezifikation muss immer gegen die konkrete OpenClaw-Version geprüft werden.

Besonders relevant sind:

- Gateway Protocol version
- `talk.session.*` schema
- Talk event schema
- provider capabilities
- audio format
- authentication requirements
- permission scopes

Nicht blind davon ausgehen, dass eine ältere OpenClaw-Version dieselbe API besitzt.

Die Unified Talk API hat ältere API-Familien ersetzt.

---

# 37. Source-Code-Stellen, die zuerst geprüft werden sollen

Der Implementierungs-Agent soll bei Unklarheiten bevorzugt den tatsächlichen OpenClaw-Source-Code untersuchen.

Besonders relevant:

    docs/gateway/protocol.md

    docs/plugins/sdk-migration.md

    docs/nodes/talk.md

    src/gateway/
        talk-*.ts

Insbesondere:

    src/gateway/talk-realtime-relay-session-create.ts

sowie die zugehörigen:

    talk-session-registry
    talk-realtime-relay-state
    talk-realtime-relay-voice
    Talk provider / transcription bridge

Die Namen können sich zwischen Releases verändern.

Immer den aktuellen Stand des verwendeten Releases priorisieren.

---

# 38. Aktueller Source-Code-Hinweis

Der aktuelle Realtime-Relay-Code erzeugt eine Gateway-owned Session und verwendet ein Audioformat mit:

    inputEncoding = pcm16
    inputSampleRateHz = 24000

Der Gateway-Code enthält außerdem eine interne Realtime-Voice-Session-Harness-/Bridge-Schicht.

Das bedeutet:

Die Zielanwendung muss NICHT die Provider-Session nachbauen.

Sie spricht nur mit:

    OpenClaw Gateway Talk API

---

# 39. Was NICHT implementiert werden soll

Nicht implementieren:

    OpenAIRealtimeClient
    DeepgramRealtimeClient
    WhisperClient
    ProviderSelector
    ProviderCredentialStore
    ProviderWebSocketManager

wenn die gewünschte Architektur Gateway-owned STT ist.

Nicht Telegram-spezifisch hartcodieren:

    Telegram -> STT

Nicht provider-spezifisch hartcodieren:

    if provider == openai
    if provider == deepgram

Nicht das Gateway-Protokoll durch einen parallelen STT-Service umgehen.

---

# 40. Zielkriterien

Die Implementierung ist funktional korrekt, wenn:

1. Die Zielanwendung kann sich am OpenClaw Gateway anmelden.

2. Sie kann:

       talk.session.create

   mit:

       mode = transcription
       transport = gateway-relay
       brain = none

   erfolgreich ausführen.

3. Sie erhält eine gültige Session-ID.

4. Sie kann PCM-Audio als Base64-Chunks über:

       talk.session.appendAudio

   übertragen.

5. Das Gateway verwendet den auf Gateway-Seite konfigurierten STT-Provider.

6. Die Zielanwendung erhält `talk.event`-Nachrichten.

7. Finale Transkription kann zuverlässig erkannt werden.

8. Die Session kann sauber beendet werden.

9. Kein STT-Provider-Credential befindet sich im Client.

10. Der Client funktioniert unabhängig davon, ob der Gateway-Administrator OpenAI, Deepgram, Groq oder einen anderen kompatiblen Provider verwendet.

---

# 41. Erweiterung: Voice Message statt Live Microphone

Wenn die Zielanwendung nicht live vom Mikrofon streamt, sondern eine fertige Voice Message besitzt:

    Voice file
       |
       v
    decode
       |
       v
    PCM16
       |
       v
    chunk
       |
       v
    talk.session.appendAudio
       |
       v
    transcript
       |
       v
    close

Damit kann auch eine fertige Voice Message über den Gateway-Streaming-STT-Pfad verarbeitet werden.

Allerdings ist zu prüfen, ob für diesen konkreten Anwendungsfall nicht der normale OpenClaw Media-Understanding-Pfad besser geeignet ist.

Die Wahl sollte sein:

    finished media attachment
        -> media understanding

    live/streaming external audio
        -> talk.session transcription

---

# 42. Erweiterung: Transcript an Agent übergeben

Wenn das Ziel nicht nur STT, sondern:

    Voice -> OpenClaw Agent

ist, soll die Implementierung nach erfolgreicher Transkription eine zweite Phase besitzen:

    Talk Transcription
          |
          v
    final transcript
          |
          v
    Agent Message Bridge
          |
          v
    OpenClaw agent/session
          |
          v
    normal agent response

Diese zweite Phase muss anhand des vorhandenen OpenClaw Gateway-Protokolls der Zielversion implementiert werden.

Nicht einfach annehmen, dass `talk.session.close` den Agent automatisch ausführt.

---

# 43. Sicherheitsmodell

Die Gateway-Verbindung ist eine privilegierte Verbindung.

Die Zielanwendung muss:

- Authentifizierung sicher speichern
- TLS/WSS verwenden, sofern Gateway nicht lokal läuft
- Tokens nicht loggen
- Session-IDs nicht als Secrets behandeln, aber trotzdem nicht unnötig exponieren
- keine Provider-Credentials erhalten
- keine `includeSecrets`-ähnlichen APIs verwenden, sofern nicht zwingend erforderlich

Die Zielanwendung sollte nur die minimal erforderlichen Gateway-Methoden verwenden.

---

# 44. Implementierungsstrategie

Der Implementierungs-Agent soll NICHT sofort die komplette Audio-Pipeline bauen.

Empfohlene Reihenfolge:

## Phase 1 – Gateway connectivity

Implementieren:

    connect
    authenticate
    generic RPC request

Test:

    talk.catalog

## Phase 2 – Session creation

Implementieren:

    talk.session.create

Test:

    mode = transcription
    transport = gateway-relay
    brain = none

## Phase 3 – Static audio

Eine bekannte PCM-Datei verwenden.

Nicht Mikrofon.

Test:

    appendAudio

## Phase 4 – Transcript events

`talk.event` vollständig loggen.

Eventtypen identifizieren.

## Phase 5 – Audio streaming

Mikrofon / echte Audioquelle anschließen.

## Phase 6 – Transcript lifecycle

Partial/final transcript sauber modellieren.

## Phase 7 – Agent integration

Erst danach Transcript in den gewünschten Agent-/Message-Pfad integrieren.

---

# 45. Minimaler Proof of Concept

Der erste POC soll möglichst klein sein:

    connect()
    authenticate()
    create transcription session
    load known PCM file
    split into chunks
    append chunks
    print talk.event
    close session

Noch NICHT:

- UI
- microphone
- provider-specific logic
- Agent response
- persistence
- reconnect
- complex buffering

Erst wenn dieser POC funktioniert, die restliche Integration darauf aufbauen.

---

# 46. Referenzen

Primäre OpenClaw-Quellen:

- Gateway Protocol:
  `docs/gateway/protocol.md`

- SDK / Plugin Migration:
  `docs/plugins/sdk-migration.md`

- Talk:
  `docs/nodes/talk.md`

- Gateway Realtime Relay:
  `src/gateway/talk-realtime-relay-session-create.ts`

Aktueller OpenClaw-Stand dokumentiert:

    talk.session.create
    talk.session.appendAudio
    talk.session.close
    talk.event

als Unified Talk API.

---

# 47. Wichtigste technische Schlussfolgerung

Die zentrale Erkenntnis dieser Recherche ist:

    Telegram ist NICHT erforderlich.

Der Gateway kann selbst als STT-Abstraktionsschicht dienen.

Die gewünschte Architektur ist:

    ┌──────────────────────┐
    │   Fremde Anwendung   │
    │                      │
    │ Microphone / Audio   │
    └──────────┬───────────┘
               │
               │ WebSocket
               │
               │ talk.session.create
               │ talk.session.appendAudio
               │
               v
    ┌──────────────────────┐
    │  OpenClaw Gateway     │
    │                      │
    │  Talk Session         │
    │  Streaming STT       │
    │  Provider Selection   │
    │  Credentials          │
    └──────────┬───────────┘
               │
               │ provider-specific
               │
               v
    ┌──────────────────────┐
    │ Configured STT       │
    │ Provider             │
    └──────────┬───────────┘
               │
               │ transcript
               v
    ┌──────────────────────┐
    │  OpenClaw Gateway     │
    └──────────┬───────────┘
               │
               │ talk.event
               v
    ┌──────────────────────┐
    │   Fremde Anwendung   │
    │                      │
    │ final transcript     │
    └──────────────────────┘

Der externe Client muss somit nur die Gateway-Talk-Schnittstelle verstehen.

Er muss nicht wissen, welcher STT-Provider dahinter arbeitet.

---

# 48. Auftrag an den Implementierungs-Agenten

Auf Basis dieses Dokuments soll der Implementierungs-Agent:

1. Die tatsächliche OpenClaw-Version der Zielumgebung feststellen.

2. Das dort tatsächlich gültige Gateway-WebSocket-Protokoll identifizieren.

3. Die vorhandene WebSocket-/RPC-Infrastruktur der Zielanwendung untersuchen.

4. `talk.session.create` implementieren.

5. `talk.session.appendAudio` implementieren.

6. `talk.event` abonnieren und auswerten.

7. `talk.session.close` implementieren.

8. Das tatsächlich erwartete Audioformat anhand des aktuellen Gateway-Responses/Source-Codes verifizieren.

9. Eine Audio-Konvertierungsschicht implementieren, falls die vorhandene Audioquelle nicht bereits PCM im erwarteten Format liefert.

10. Einen minimalen Static-PCM-POC erstellen.

11. Erst danach Live-Audio anschließen.

12. Partial- und Final-Transcript sauber unterscheiden.

13. Fehler, Session-Lifecycle und Reconnect behandeln.

14. Provider-spezifische STT-Logik NICHT in die Zielanwendung übernehmen.

15. Falls das endgültige Ziel `Voice -> OpenClaw Agent -> Response` ist, separat den Message-/Agent-Routing-Pfad untersuchen und implementieren.

---

# 49. Dinge, die vor der finalen Implementierung verifiziert werden müssen

Diese Punkte sind absichtlich nicht als unveränderliche Fakten festgeschrieben:

- exakte Gateway-Handshake-Payload
- Authentifizierungsablauf
- exakte RPC-Envelope
- exakte `talk.event`-Payloads
- exakte Partial/Final-Eventtypen
- exakte Close-/Finalization-Semantik
- Audio-Kanalanzahl
- Endianness
- exakte Chunkgröße
- Timeout-Verhalten
- Session-TTL
- Permission Scopes
- Verhalten bei Gateway-Reconnect
- Verhalten bei Provider-Ausfall
- Übergang von finalem Transcript zu normalem Agent Run

Diese Informationen müssen aus dem tatsächlichen OpenClaw-Stand der Zielumgebung abgeleitet werden.

---

# 50. Primäre Designregel

Die Implementierung soll sich an folgendem Prinzip orientieren:

    External Application
            |
            | generic audio
            v
    OpenClaw Gateway
            |
            | provider-specific STT
            v
    STT Provider

und NICHT:

    External Application
            |
            +--> Telegram-specific logic
            +--> OpenAI-specific logic
            +--> Deepgram-specific logic
            +--> Whisper-specific logic

Das Gateway soll die Provider-Abstraktion besitzen.

Die fremde Anwendung soll lediglich eine generische OpenClaw-Gateway-Talk-Integration implementieren.