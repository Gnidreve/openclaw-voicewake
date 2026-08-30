Implementiere die Audioausgabe über die native OpenClaw-TTS-Pipeline.

Voraussetzung:
- Der TTS-Provider ist bereits vollständig konfiguriert.
- Es soll kein eigener TTS-Provider eingerichtet oder ausgewählt werden.
- Verwende die OpenClaw-eigene Gateway-TTS-Schnittstelle.

Ziel:
Wenn der Agent eine Antwort erzeugt, soll diese optional zusätzlich als Audio ausgegeben werden.

Ablauf:
1. Der Agent erzeugt eine normale Textantwort.
2. Der Text wird an das OpenClaw Gateway über dessen TTS-Schnittstelle übergeben.
3. Das Gateway verwendet den bereits konfigurierten TTS-Provider.
4. Die erzeugten Audiodaten werden vom Gateway zurückgegeben.
5. Die Anwendung soll die Audiodaten entgegennehmen und an den vorgesehenen Audio-Output weiterreichen.
6. Text- und Audioausgabe sollen dabei logisch dieselbe Agentenantwort repräsentieren.

Technische Vorgabe:
- Nutze die native Gateway-Operation `tts.speak`.
- Übergib mindestens den vollständigen Antworttext als `text`.
- Berücksichtige den vom Gateway zurückgegebenen `audioBase64`-Wert.
- Verwende die ebenfalls zurückgegebenen Audio-/MIME-Informationen, um das Audio korrekt weiterzuverarbeiten.
- Nicht selbst direkt den TTS-Provider aufrufen.
- Keine zusätzliche TTS-Abstraktion bauen, wenn OpenClaw diese bereits bereitstellt.
- Fehler bei der TTS-Erzeugung dürfen die normale Textantwort nicht unbrauchbar machen.

Erwarteter Datenfluss:

Agent
  ↓
Textantwort
  ↓
OpenClaw Gateway: tts.speak
  ↓
{
  audioBase64,
  mimeType,
  format,
  provider,
  ...
}
  ↓
Base64 dekodieren
  ↓
Audio-Output

Wichtig:
- Prüfe zuerst die im aktuellen OpenClaw-Projekt vorhandene Gateway-/TTS-Implementierung und deren tatsächliche API-Strukturen.
- Erfinde keine Gateway-Methoden, Parameter oder Response-Felder.
- Orientiere dich an der im Projekt verwendeten OpenClaw-Version.
- Bestehende Audio-, Streaming- und Session-Logik des Projekts soll nach Möglichkeit wiederverwendet werden.
- Die Implementierung soll so aufgebaut sein, dass später Streaming bzw. kontinuierliche Sprachinteraktion ergänzt werden kann.
