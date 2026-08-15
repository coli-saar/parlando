# Parlando: Unterlage zur datenschutzrechtlichen Plattformbewertung

**Adressat:** Datenschutzbeauftragter der Universität des Saarlandes<br>
**Prüfgegenstand:** Parlando, selbst gehostetes Standardverfahren für wissenschaftliche Online-Experimente<br>
**Privacy Contract:** Version 1<br>
**Stand der Unterlage:** 14. August 2026<br>
**Softwareversion bei Prüfung:** [einzutragen]

## 1. Bitte um Plattformbewertung

Sehr geehrte Damen und Herren,

wir bitten um die datenschutzrechtliche Bewertung der Forschungsplattform Parlando in dem nachfolgend abgegrenzten Standardbetrieb. Die Bewertung soll nicht ein einzelnes Experiment freigeben. Sie soll die gleichbleibenden technischen Verarbeitungsvorgänge der Plattform beurteilen, damit

- Experimente an der Universität des Saarlandes nur noch in ihren fachlichen und organisatorischen Besonderheiten geprüft werden müssen und
- andere Hochschulen bei einer eigenen, selbst gehosteten Installation auf die technische Beschreibung und Risikobewertung verweisen können.

Andere Hochschulen bleiben für ihre Verarbeitung selbst verantwortlich. Die Stellungnahme des UdS-DSB ersetzt weder deren lokale Datenschutzprüfung noch deren Verzeichnis von Verarbeitungstätigkeiten, Rechtsgrundlagenentscheidung oder gegebenenfalls erforderliche Freigabe. Sie soll diese Prüfung auf die tatsächlich lokalen Angaben reduzieren.

Wir bitten insbesondere um Stellungnahme zu folgenden Punkten:

1. Ist die beschriebene Plattformversion für den Standardbetrieb datenschutzgerecht gestaltet?
2. Ist die nachfolgende Abgrenzung zwischen allgemein bewertbarer Plattform und lokal zu prüfendem Experiment sachgerecht?
3. Teilt der DSB die Einschätzung, dass aus dem Standardbetrieb allein regelmäßig keine Pflicht zur Durchführung einer DSFA folgt?
4. Ist das Verfahren für die Nachnutzung pseudonymisierter Forschungsdaten und die Erzeugung eines Kandidaten für ein anonymisiertes Veröffentlichungskorpus angemessen?
5. Sind die englische Teilnehmerinformation und die dazugehörigen Consent-Items als wiederverwendbare Ausgangstexte geeignet?
6. Welche Änderungen sollen eine neue Bewertung des Parlando Privacy Contract auslösen?

## 2. Dokumentstatus und Bestandteile der Einreichung

Diese Hauptunterlage beschreibt Zweck, Funktionsweise, Datenverarbeitung, Risiken und organisatorische Abgrenzung der zur Prüfung vorgelegten Plattformversion vollständig. Für ihr Verständnis sind keine weiteren Projekt- oder Entwicklungsdokumente erforderlich. Die Anlagen dienen ausschließlich dazu, die konkrete Softwareversion und Konfiguration zu belegen und die tatsächlich verwendeten Teilnehmertexte vorzulegen.

Für die abschließende Plattformbewertung wird die Hauptunterlage mit folgenden ausdrücklich für das Prüfverfahren bestimmten Anlagen eingereicht:

| Anlage | Titel und Inhalt | Zweck |
| --- | --- | --- |
| A | **Versions- und Datenschutzstatus der geprüften Installation**: Parlando-Version, Git-Revision, `privacy_contract_version`, aktive Speicherschalter, gespeicherte Datenarten, konfigurierte externe Dienste, Exportvarianten und Verfügbarkeit der Teilnehmerlöschung | legt den konkreten technischen Prüfgegenstand fest |
| B | **Technischer Abnahmenachweis für Parlando Privacy Contract Version 1**: Prüfer, Datum, Revision, Prüfergebnisse zu den in Abschnitt 21 genannten Plattform- und Sicherheitsgarantien | belegt, dass die in dieser Hauptunterlage beschriebenen Funktionen in der geprüften Version vorhanden sind |
| C | **Participant Information and Privacy Notice** in der geprüften englischen Plattformfassung | legt den wiederverwendbaren Ausgangstext für die Information von Versuchspersonen vor |
| D | **Consent Items** in der zu Anlage C passenden englischen Plattformfassung | legt die zugehörigen Erklärungen und ihre technische Konfiguration vor |

Vor einer abschließenden Stellungnahme werden Softwareversion und Privacy-Contract-Version im Kopf dieser Unterlage eingetragen und die Anlagen A bis D beigefügt. Ändert sich der Prüfgegenstand während des Verfahrens, werden die betroffenen Anlagen aktualisiert. Eine Anlage darf keine zusätzliche Verarbeitung einführen, die nicht bereits in dieser Hauptunterlage beschrieben ist.

## 3. Zweck und Einsatzgebiet von Parlando

Parlando ist eine Forschungsplattform zur Entwicklung, Durchführung und Auswertung kontrollierter browserbasierter Dialogexperimente. Zwei Personen oder eine Person und ein Software-Agent bearbeiten gemeinsam eine fiktive Aufgabe. Die Rollen können unterschiedliche Informationen besitzen, sehen jeweils nur die für sie bestimmte Sicht auf die Aufgabe und können den gemeinsamen Zustand durch Spielhandlungen verändern. Je nach Experiment koordinieren sie sich zusätzlich über Tastatur-Chat oder Live-Sprache.

Parlando erhebt nicht gezielt die reale Lebenssituation oder Identität der Versuchsperson. Untersucht wird ihr beobachtbares Handeln und Kommunizieren innerhalb der kontrollierten Aufgabe. Parlando ermöglicht insbesondere Untersuchungen dazu,

- wie Personen mit verteilten oder unvollständigen Informationen kooperieren,
- wie Dialog und Handlungen im Aufgabenverlauf zusammenwirken,
- welche Kommunikationsstrategien zu einem bestimmten Spielergebnis führen und
- wie sich Mensch–Mensch- und Mensch–Agent-Interaktionen unter denselben Spielregeln unterscheiden.

Hierfür führt die Plattform den zeitlichen Zusammenhang zwischen Rolle, rollenbezogener Information, Spielhandlung, Kommunikation, Zustandsänderung und Ergebnis in einem strukturierten Sitzungsdatensatz zusammen. Diese Verknüpfung ist die zentrale wissenschaftliche Funktion von Parlando. Eine reine Chatplattform würde den Aufgabenverlauf nicht abbilden; eine reine Spielplattform würde die Kommunikation nicht mit den Zustandsänderungen verbinden.

Parlando gibt keinen einzelnen Forschungszweck und keine einzelne Forschungsfrage verbindlich vor. Diese werden für jedes Experiment von der einsetzenden Hochschule festgelegt. Die Plattformbewertung betrifft die gleichbleibenden technischen Mittel und den unten beschriebenen Rahmen zulässiger Forschungszwecke.

## 4. Funktionsweise und Lebenszyklus eines Experiments

Ein typisches Experiment läuft aus Sicht einer Versuchsperson wie folgt ab:

1. Die Person öffnet die von der verantwortlichen Hochschule betriebene Studienseite.
2. Sie liest die versionierte Teilnehmerinformation und gibt die erforderlichen Erklärungen ab.
3. Parlando erzeugt automatisch eine zufällige, menschenlesbare Teilnehmerkennung für dieses Experiment. Die Person wird nicht nach einem Namen gefragt. Eine gegebenenfalls zur Rekrutierung oder Vergütung benötigte externe Kennung wird davon getrennt behandelt. Dieselbe externe Kennung erhält in einem anderen Experiment eine unabhängig erzeugte Teilnehmerkennung.
4. Die Person wird einer Sitzung, einem Raum und einer Rolle zugeordnet. Die Gegenrolle wird von einer zweiten Versuchsperson oder einem vertrauenswürdigen Software-Agenten übernommen.
5. Der Server sendet jeder Rolle nur die für sie bestimmte Beobachtung des fiktiven Spielzustands. Verdeckte Informationen der anderen Rolle werden serverseitig nicht in diese Ansicht aufgenommen.
6. Der Browser übermittelt vorgeschlagene Spielhandlungen an den Server. Erst der Server prüft, ob die Handlung nach den Regeln des Experiments zulässig ist, verändert den gemeinsamen Zustand und verteilt die daraus folgende rollenbezogene Ansicht.
7. Optional übermittelt Parlando Tastatur-Chat oder Live-Audio zwischen den Rollen. Bei aktivierter Transkription wird Sprache zusätzlich in Echtzeit in Text umgewandelt. Finale Transkripte können anschließend denselben Dialog- und Agentenpfad wie getippte Nachrichten verwenden.
8. Die aktivierten Handlungen, Zustandsänderungen, Kommunikationsinhalte, Zeitangaben und Ergebnisse werden als geordnete Sitzungsdaten in der lokalen Datenbank gespeichert.
9. Am Ende erzeugt der Experimentcode eine strukturierte Abschlusszusammenfassung, beispielsweise Erfolg, Ergebnis oder für die Forschungsfrage benötigte Zustandsmerkmale.
10. Die Versuchsperson kann die Sitzung verlassen; Forschende arbeiten anschließend mit den gespeicherten Daten und nicht mit einem nur im Browser vorhandenen Zustand.

Die Forschenden verwenden Parlando anschließend, um laufende und abgeschlossene Sitzungen im geschützten Adminbereich zu kontrollieren, technisch unbrauchbare oder abgebrochene Sitzungen zu erkennen, Forschungsdaten zu exportieren und die zusammengehörigen Handlungs-, Dialog- und Ergebnisverläufe auszuwerten. Für Nachnutzung oder Veröffentlichung wird aus dem pseudonymisierten Forschungsbestand ein gesonderter Korpuskandidat erzeugt und vor einer öffentlichen Freigabe inhaltlich geprüft.

## 5. Bewertungsgegenstand: Plattformkern, Experimentcode und lokaler Betrieb

Parlando trennt wiederverwendbare Infrastruktur von der inhaltlichen Ausgestaltung eines Experiments:

| Ebene | Festgelegte Funktion | Bedeutung für die Bewertung |
| --- | --- | --- |
| Parlando-Plattformkern | Teilnehmerverwaltung und Erklärungsnachweis, Räume, Rollen, Sitzungen, autoritative Handlungsvalidierung, rollenbezogene Übermittlung, optionale Text-/Sprachkanäle, Persistenz, Adminbereich, Export und Löschfunktion | Gegenstand der wiederverwendbaren Plattformbewertung und des Privacy Contract |
| Experimentcode und -konfiguration | fiktive Aufgabe, Forschungsfrage, Rollenwissen, erlaubte Handlungen, Zustands- und Ergebnisfelder, Darstellung im Browser, Agentenstrategie und aktivierte Speicher-/Sprachfunktionen | lokal darauf zu prüfen, dass das Experiment innerhalb des Standardbetriebs bleibt |
| Lokaler Hochschulbetrieb | Verantwortlicher, Rekrutierung, Rechtsgrundlage, Hosting, Auftragsverarbeiter, Zugriffsberechtigte, Aufbewahrung und konkrete Veröffentlichung | von jeder einsetzenden Hochschule selbst festzulegen und zu dokumentieren |

Der Plattformkern besteht aus folgenden Komponenten:

- **Teilnehmerbrowser:** zeigt Information, Warteraum, rollenbezogene Aufgabe und Kommunikation an und sendet Eingaben; er ist nicht die maßgebliche Instanz für Spielregeln oder verdeckte Informationen.
- **Parlando-Server:** authentisiert Zugriffe, ordnet Räume und Rollen zu, prüft Handlungen, berechnet über den Experimentcode neue Zustände, vermittelt Kommunikation und schreibt aktivierte Forschungsdaten.
- **Experimentmodul:** definiert typisiert Zustand, Handlung, rollenbezogene Beobachtung, Ereignisse und Abschlusszusammenfassung der jeweiligen fiktiven Aufgabe.
- **Lokale SQLite-Datenbank:** enthält die dauerhaften Experiment-, Teilnehmer-, Sitzungs-, Erklärungs- und Ereignisdatensätze, aus denen Monitoring und Exporte erzeugt werden.
- **Adminbereich:** dient berechtigten Forschenden zur Sitzungsübersicht, zum Export, zur Datenschutzstatusanzeige und zur manuellen Teilnehmerlöschung.
- **Optionale Dienste:** vertrauenswürdige lokale oder angebundene Agenten, Speechmatics für Live-Transkription und ElevenLabs für die Sprachausgabe nicht personenbezogenen Agententextes.

Ein neues Spiel verändert den Privacy Contract nicht allein dadurch, dass es andere fiktive Zustände, Handlungen oder Darstellungen verwendet. Eine zusätzliche Bewertung wird aber erforderlich, wenn es den Standardbetrieb verlässt, neue personenbezogene Datenarten oder Empfänger einführt oder die in dieser Unterlage bewerteten Plattformgarantien umgeht.

## 6. Abgegrenzter Standardbetrieb

Die erbetene Plattformbewertung ist auf folgende Verwendung beschränkt:

- volljährige, freiwillig teilnehmende Versuchspersonen,
- wissenschaftliche Experimente mit fiktiven Spiel- und Dialogaufgaben,
- zwei menschliche Rollen oder menschliche Rollen und vertrauenswürdige Software-Agenten,
- Spielhandlungen, technische Zeitpunkte und Ergebnisse,
- optional Tastatur-Chat,
- optional Live-Sprache zwischen den Rollen,
- optional Echtzeittranskription mit Speechmatics,
- wissenschaftliche Auswertung und Nachnutzung pseudonymisierter Forschungsdaten,
- Erzeugung und nachfolgende inhaltliche Prüfung von Kandidaten für anonymisierte Spiel- und Dialogkorpora und
- Selbsthosting durch die jeweils verantwortliche Hochschule.

Nicht vom Standardbetrieb umfasst sind:

- gezielte Erhebung von Gesundheitsdaten oder anderen besonderen Kategorien nach Art. 9 DSGVO,
- Minderjährige oder Personengruppen, bei denen die Freiwilligkeit nicht ohne Weiteres gewährleistet ist,
- biometrische Identifizierung oder Sprechererkennung,
- Persönlichkeits-, Leistungs- oder Eignungsbewertung mit Folgen für die Versuchsperson,
- Täuschung über datenschutzrelevante Verarbeitungen,
- dauerhafte Speicherung von Roh-Audio,
- Einbindung eines fremdbetriebenen Modell- oder Agentendienstes, der Teilnehmerdaten zu eigenen Zwecken verarbeitet,
- zentrale Verarbeitung der Forschungsdaten durch die Universität des Saarlandes für andere Hochschulen oder
- öffentliche Bereitstellung lediglich pseudonymisierter, weiterhin personenbezogener Rohdaten.

Ein Experiment außerhalb dieser Grenzen benötigt eine eigene Bewertung, unabhängig davon, ob es technisch mit Parlando durchgeführt wird.

## 7. Rollen bei Selbsthosting

Die einsetzende Hochschule betreibt Parlando auf eigener Infrastruktur oder in einer von ihr ausgewählten Auftragsverarbeitung. Sie bestimmt Forschungszweck, Datenumfang, Rekrutierung, Rechtsgrundlage, Aufbewahrung und Veröffentlichung. Sie ist Verantwortliche im Sinne von Art. 4 Nr. 7 DSGVO.

Die Universität des Saarlandes und die Entwickler von Parlando erhalten durch die bloße Bereitstellung der Software keinen Zugriff auf die Installation oder Forschungsdaten der einsetzenden Hochschule. Sie sind deshalb in diesem Modell weder Empfänger noch Auftragsverarbeiter der dort erhobenen Daten.

Die jeweilige Hochschule prüft selbst:

- ihre lokale Hostingumgebung,
- gegebenenfalls eingesetzte Infrastruktur-Auftragsverarbeiter,
- ihren Speechmatics-Vertrag bei aktivierter Transkription,
- Rekrutierungs- und Vergütungsdienste,
- ihre Rechtsgrundlage und Teilnehmerinformation sowie
- Aufbewahrung, manuelle Löschung und Veröffentlichung.

Die Rollen folgen der tatsächlichen Verarbeitung. Sollte künftig ein zentraler Parlando-Dienst angeboten oder sollten Daten gemeinsam für hochschulübergreifende Zwecke verarbeitet werden, ist die Verantwortlichkeit neu zu bestimmen.

## 8. Plattform und Datenfluss

Parlando ist eine browserbasierte Forschungsplattform. Der Server ordnet Rollen zu, verwaltet eine Sitzung, validiert Spielhandlungen, verteilt rollenbezogene Ansichten und speichert die für das Experiment aktivierten Forschungsdaten in SQLite.

```text
Browser A ──HTTPS/WSS──> selbst gehosteter Parlando-Server ──> lokale SQLite-Datenbank
   ▲                              │                                  │
   │                              ├── Rollenansicht/Text/Audio ─────> Browser B
   │                              ├── Live-Audio ───────────────────> Speechmatics [optional]
   │                              ├── Rollenansicht/Text ───────────> vertrauenswürdiger Agent
   │                              └── nur Agententext ──────────────> ElevenLabs TTS [optional]
   │
   └────────────── Rollenansicht/Text/Audio aus dem gemeinsamen Raum
```

### 8.1 Andere Versuchsperson

Die jeweils andere Rolle erhält die für das Spiel vorgesehenen Handlungen und Kommunikationsinhalte. Bei Sprache hört sie die Live-Stimme; bei Chat erhält sie den Text. Dies ist ein offener Bestandteil des Experiments. Parlando kann eine eigenständige Aufzeichnung durch die andere Person nicht technisch vollständig verhindern. Die Teilnehmerinformation untersagt deshalb Aufzeichnung und Weitergabe.

### 8.2 Vertrauenswürdige Agenten

Lokale und über die Parlando-Schnittstelle angebundene Remote-Agenten sind Bestandteil des kontrollierten Experimentcodes. Sie erhalten nur die rollenbezogene Spielsicht und die für ihre Rolle bestimmten Nachrichten beziehungsweise Transkripte. Sie gelten im Standardbetrieb nicht als externer Empfänger. Ein Agentendienst, der außerhalb der Kontrolle der verantwortlichen Hochschule eigene Zwecke verfolgt, fällt nicht unter diese Bewertung.

### 8.3 Speechmatics

Bei aktivierter Echtzeittranskription überträgt Parlando das Live-Mikrofonsignal an Speechmatics und erhält Text und Zeitinformationen zurück. Parlando speichert kein Roh-Audio. Die einsetzende Hochschule muss Vertragsrolle, Region, Aufbewahrung, Unterauftragsverarbeiter und gegebenenfalls Drittlandübermittlungen anhand ihres eigenen Accounts prüfen.

### 8.4 ElevenLabs

Die TTS-Schnittstelle übermittelt ausschließlich den vom Software-Agenten erzeugten Text und die technische Voice-/Modellkonfiguration. Mikrofon-Audio, Teilnehmernachrichten, Transkripte, Teilnehmer- oder Sitzungskennungen und Spielzustände sind nicht Bestandteil des TTS-Aufrufs. Der Standardbetrieb setzt außerdem voraus, dass der Agent keine realen Angaben einer Versuchsperson in seiner Ausgabe wiedergibt. Unter dieser technischen und experimentellen Grenze ist ElevenLabs kein Empfänger von Versuchspersonendaten.

## 9. Datenarten, Erforderlichkeit und Speicherschalter

Der Standardbetrieb verwendet automatisch erzeugte, studienbezogene Teilnehmerkennungen. Rekrutierungskennungen sind nicht Bestandteil des normalen Forschungs- oder Korpusexports. Ein Eingabefeld und ein Datenbankfeld für Anzeigenamen bestehen nicht.

| Datenart | Verarbeitung im Standardbetrieb | Funktionaler beziehungsweise wissenschaftlicher Bedarf |
| --- | --- | --- |
| Teilnehmerzuordnung | experimentbezogene interne ID und zufällig erzeugte, menschenlesbare Teilnehmerkennung; optionale Rekrutierungszuordnung nur für lokalen Rekrutierungs-/Vergütungsprozess | ordnet Erklärungen und zusammengehörige Sitzungen innerhalb eines Experiments zu, ohne experimentübergreifendes Teilnehmerpseudonym oder Klarnamen im Forschungsdatensatz |
| Agentenzuordnung | beschreibende Kennung aus Agententyp, Implementierungsname, soweit vorhanden, und Version; bei fehlender Versionsangabe ausdrückliche Markierung `unversioned` | macht in Administration und Forschungsdaten unmittelbar erkennbar, welche kontrollierte Agentimplementierung gehandelt oder kommuniziert hat |
| Sitzungsdaten | zufällig erzeugte, menschenlesbare Dialogkennung, Experiment, Raum, Rolle, Verbindungsstatus und Zeitpunkte | verbindet die zwei Rollen und ordnet Ereignisse in einem gemeinsamen Ablauf |
| Spielhandlungen und Ergebnisse | Aktionen, Ereignisse und Abschlussdaten entsprechend dem Experimentcode | bildet den untersuchten Aufgabenverlauf und seinen Ausgang ab |
| Vollständiger Spielzustand | nur wenn `store_full_game_state: true` | ermöglicht, falls für die Forschungsfrage erforderlich, die Rekonstruktion der Situation, in der eine Handlung oder Äußerung erfolgte |
| Tastatur-Chat | bestehende `conversation_message`-Ereignisse mit Ursprung `typed`; dauerhafte Speicherung nur wenn `store_typed_messages: true` | ermöglicht die Analyse des Zusammenhangs zwischen sprachlicher Koordination und Spielverlauf |
| Finale Sprachtranskripte | ein `conversation_message`-Ereignis mit Herkunft `voice_transcript` und Zeit-/Provider-Metadaten; Speicherung nur wenn `store_final_transcripts: true` | macht gesprochene Kommunikation für dieselben Dialoganalysen zugänglich, ohne Text doppelt oder Roh-Audio zu archivieren |
| Sprachdiagnostik | minimierte Fehlercodes und technische Messwerte nur wenn `store_voice_diagnostics: true`; keine Mikrofon-Geräte-ID, Gerätebezeichnung oder freien Fehlertexte | erlaubt, technisch fehlerhafte Sprachsitzungen zu erkennen und wissenschaftlich begründete Ausschlüsse vorzunehmen |
| Erklärungsnachweis | Entscheidungen, Zeitpunkt, Informationsversion und -URL sowie serverseitiger Hash aus dieser Referenz und der geordneten Consent-Darstellung | belegt, welche konfigurierte Information referenziert und welche Erklärung abgegeben wurde |
| Softwareprovenienz | Parlando-, Spiel- und Agentenversion sowie Privacy-Contract-Version | ermöglicht Reproduzierbarkeit und die Zuordnung der Daten zum geprüften technischen Verhalten |

Die vorgesehenen Schalter lauten:

```yaml
privacy:
  store_full_game_state: true
  store_typed_messages: true
  store_final_transcripts: true
  store_voice_diagnostics: false
```

Die Schalter ändern nicht die bestehende Darstellung von Nachrichten, Transkripten oder Ereignissen in der Datenbank. Sie bestimmen nur, ob die betreffende Datenart dauerhaft geschrieben wird.

## 10. Daten, die Parlando nicht dauerhaft speichert

Im Standardbetrieb speichert Parlando nicht:

- Mikrofon-Rohsignal,
- partielle Transkriptionshypothesen,
- erzeugtes TTS-Audio,
- Mikrofon-Geräte-ID oder Gerätebezeichnung,
- frei formulierte Browser-/Gerätefehlertexte,
- Teilnehmerdaten in TTS-Anfragen oder
- Telemetrie an die Universität des Saarlandes beziehungsweise das Parlando-Projekt.

Infrastruktur-, Proxy- und Systemprotokolle der selbst gehosteten Installation sind nicht Bestandteil der Parlando-Anwendungsdaten und werden lokal bewertet.

## 11. Forschungsnachnutzung und Exporte

Wissenschaftliche Nachnutzung und die Veröffentlichung anonymisierter Korpora sind Standardzwecke. Sie werden nicht als nachträglicher optionaler Zusatz behandelt, sondern vor Teilnahme klar in der Teilnehmerinformation beschrieben.

Parlando stellt drei feste Exportvarianten bereit:

| Export | Zweck und Inhalt |
| --- | --- |
| `research` | experimentbezogene Teilnehmer- und Dialogkennungen sowie aktivierte Spiel-, Nachrichten-, Transkript- und Ergebnisdaten für wissenschaftliche Auswertung, Reproduzierbarkeit und Nachnutzung; solange eine Zuordnung möglich ist, handelt es sich um pseudonymisierte personenbezogene Daten |
| `corpus` | veröffentlichungsorientierter Korpuskandidat mit denselben experimentbezogenen Zufallskennungen, ohne interne Systemkennungen und ohne absolute Zeitstempel; noch nicht allein durch den Export anonym |
| `full` | vollständiger interner Bestand für Administration und Bearbeitung von Betroffenenanfragen |

`research` und `corpus` verwenden feste Feld-Allowlists. Externe Rekrutierungskennungen, Erklärungsnachweise, vollständige Konfiguration und Zugangsdaten erscheinen dort nicht.

Menschliche Teilnehmer und Dialoge erhalten ihre zufälligen, menschenlesbaren Kennungen bereits bei der Anlage in Parlando. Agententeilnehmer erhalten stattdessen eine beschreibende Kennung aus Agententyp, Implementierungsname, soweit vorhanden, und Version, beispielsweise `agent:space_game.back_and_forth:BackAndForthAgent@0.2.0`. Eine fehlende Version wird als `unversioned` ausgewiesen. `research` und `corpus` verwenden dieselben Kennungen bei jedem Export desselben Experiments. Dadurch lassen sich wiederholte Exporte dieses Experiments zusammenführen und Agentimplementierungen unterscheiden, ohne interne Datenbank-, Raum-, Sitzungs- oder Nachrichtenkennungen offenzulegen. Eine wiederkehrende Rekrutierungskennung wird je Experiment mit einem neuen unabhängigen menschlichen Teilnehmernamen verknüpft; Parlando stellt somit keine experimentübergreifende Forschungskennung bereit. Die Endwörter der Zufallsnamen stammen aus getrennten Listen (Tiernamen für menschliche Teilnehmer, Orts- und Objektnamen für Dialoge), sodass ihre Kennungsart ohne Präfix erkennbar ist. Der `corpus`-Export entfernt zusätzlich absolute Zeitstempel zugunsten relativer Abstände.

Diese strukturelle Bearbeitung allein garantiert keine Anonymität freier Äußerungen. Vor Veröffentlichung wird das Korpus deshalb inhaltlich auf Namen, Kontaktangaben, reale Orte/Institutionen, seltene identifizierende Details und Angaben über Dritte geprüft. Bis zum Abschluss dieser Prüfung ist der Export als `corpus_candidate` gekennzeichnet. Kann eine Identifizierbarkeit nicht hinreichend ausgeschlossen werden, erfolgt keine öffentliche Veröffentlichung; gegebenenfalls ist nur kontrollierter Zugang zu pseudonymen Daten zulässig.

Eine Veröffentlichung als anonymes Korpus erfolgt erst, wenn die Rekrutierungszuordnung und sonstige realistisch nutzbare Zuordnungsmittel entfernt sind und die Inhaltsprüfung keine hinreichende Identifizierbarkeit mehr ergibt. Die zufälligen menschlichen Teilnehmer- und Dialogkennungen sowie die beschreibenden Agentenkennungen können dabei als reine Korpuslabels erhalten bleiben. Nach dieser Anonymisierung kann die Forschungseinrichtung einzelne Beiträge nicht mehr einer Person zuordnen oder nachträglich personenbezogen aus dem veröffentlichten Korpus entfernen. Dies wird vor der Teilnahme transparent erklärt.

## 12. Teilnehmerinformation und Erklärungsnachweis

Parlando zeigt vor Beginn eine versionierte Teilnehmerinformation an beziehungsweise verlinkt sie dauerhaft. Der Server speichert mit der Erklärung:

- die Versionskennung der Teilnehmerinformation,
- den serverseitig berechneten Hash aus Versionskennung, Informations-URL und geordneter Consent-Darstellung,
- die Entscheidung zu jedem Consent-Item und
- den Zeitpunkt der Erklärung.

Eine Textänderung erzeugt eine neue Version. Der Raumbeitritt wird erst nach erfolgreicher Speicherung aller erforderlichen Entscheidungen ermöglicht. Parlando lädt den Inhalt der verlinkten Informationsseite nicht selbst und hasht ihn daher nicht. Die verantwortliche Stelle archiviert die tatsächlich veröffentlichte Seite unter der gespeicherten Versionskennung; Anlage C enthält diese Fassung für die Plattformprüfung.

Die geprüfte englische Teilnehmerinformation wird als Anlage C und die dazu passende Konfiguration der Consent-Items als Anlage D eingereicht. Beide Anlagen tragen eine eigene Versionskennung. Bei einer späteren lokalen Einführung ersetzt die einsetzende Hochschule Platzhalter und lokale Angaben und vergibt für ihre veröffentlichte Fassung eine nachvollziehbare lokale Version.

Die Texte verwenden Einwilligung als Rechtsgrundlage. Eine einsetzende Hochschule, die sich auf eine öffentliche Aufgabe oder eine andere Rechtsgrundlage stützt, muss den entsprechenden Abschnitt sowie die Datenverarbeitungs- und gegebenenfalls Voice-Consent-Items lokal ersetzen. Die freiwillige Teilnahmeerklärung bleibt davon getrennt.

## 13. Manuelle Teilnehmerlöschung

Parlando führt keine automatische Löschung von Forschungsdaten aus. Die verantwortliche Hochschule legt ihre Aufbewahrungsentscheidung fest und benennt die für die manuelle Umsetzung verantwortliche Person.

Im Adminbereich steht „Delete participant data“ zur Verfügung. Vor der Ausführung zeigt die Funktion die experimentbezogene Teilnehmerkennung, betroffene Sitzungen sowie die Anzahl der Identitäts-, Erklärungs-, Nachrichten-, Transkript- und Ereignisdatensätze. Nach Bestätigung:

- werden externe Kennung, Teilnehmerkennung und Teilnehmermetadaten entfernt,
- werden Erklärungsdatensätze sowie verfasste Nachrichten und Transkripte nach der lokal festgelegten Verfahrensregel gelöscht,
- werden Teilnehmerverweise in gemeinsam benötigten, rein fiktiven Spielereignissen durch `deleted_participant` ersetzt und
- erscheinen Identitätszuordnung, Erklärungsdatensätze und gelöschte Kommunikationsinhalte nicht mehr in späteren Exporten; gemeinsam benötigte Spielereignisse bleiben nur mit entferntem Teilnehmerbezug erhalten.

Diese Funktion wahrt den gemeinsamen Spielverlauf und die Daten der anderen Rolle, ohne eine rückführbare Teilnehmerkennung beizubehalten. Die konkrete Reaktion auf Widerruf, Widerspruch oder Löschverlangen richtet sich nach der lokal gewählten Rechtsgrundlage und den anwendbaren Forschungsausnahmen.

Da die Teilnehmerkennung nur innerhalb eines Experiments gilt, entfällt mit der Löschung ihrer Rekrutierungszuordnung und der internen Teilnehmerkennung die von Parlando bereitgestellte Identifizierungsmöglichkeit. Dies lässt die gesondert zu prüfenden Grenzen unberührt: noch vorhandene Backups oder außerhalb Parlandos geführte Zuordnungslisten sowie identifizierende Angaben im freien Dialoginhalt können weiterhin eine Zuordnung ermöglichen und müssen nach dem lokalen Lösch- und Veröffentlichungskonzept behandelt werden.

## 14. Datenschutzstatus der Installation

Der Adminbereich erzeugt einen als Markdown oder JSON herunterladbaren Datenschutzstatus. Er enthält keine Secrets und weist aus:

- Parlando-Version und `privacy_contract_version`,
- aktive Speicherschalter und gespeicherte Datenarten,
- konfigurierte externe Dienste und deren Datenfluss,
- Nicht-Speicherung von Roh-Audio,
- verfügbare Exportvarianten und
- Verfügbarkeit der manuellen Teilnehmerlöschung.

Der Status behauptet keine organisatorisch nicht erkennbaren Tatsachen. Insbesondere werden Selbsthosting, Verantwortlicher, Rechtsgrundlage, Aufbewahrung und Vertragsstatus nicht automatisch abgeleitet. Diese Angaben macht die einsetzende Hochschule in ihrem lokalen Beiblatt.

Die DSB-Stellungnahme kann damit auf `privacy_contract_version: 1` bezogen werden. Die Privacy-Contract-Version wird nur geändert, wenn sich Datenarten, externe Empfänger, Export-/Anonymisierungsverhalten, Teilnehmerlöschung oder Informations-/Einwilligungsnachweis materiell ändern. Release Notes weisen aus, ob der Privacy Contract unverändert geblieben ist.

## 15. Sicherheitsgarantien der geprüften Plattform

Die zur Prüfung vorgelegte Plattformversion setzt folgende Sicherheitsgarantien technisch durch:

- anwendungsseitig geschützte Admin- und Exportfunktionen,
- Trennung öffentlicher Teilnehmerkennungen von geheimen Berechtigungsnachweisen,
- serverseitig bestimmte Teilnehmerquelle und Identität,
- restriktive HTTP- und WebSocket-Origin-Prüfung,
- Größen-, Raten-, Laufzeit- und Ressourcenbegrenzungen,
- dauerhafte kritische Ereignisse vor sichtbarer Zustandsänderung,
- Experimentpersistenz und Exporte ohne Zugangsdaten oder sonstige Geheimnisse,
- verschlüsselter und authentisierter Transport zu Remote-Agenten und
- gehärtete, reproduzierbare Auslieferungsartefakte.

Anlage B dokumentiert für jede dieser Garantien Prüfmethode, Ergebnis und geprüfte Revision. Die Bewertung der Plattform ersetzt nicht die Prüfung der lokalen Infrastruktur. Datenträgerverschlüsselung, Backup, Betriebssystem- und Datenbankadministration, Berechtigungsvergabe sowie Aufbewahrung von Infrastrukturprotokollen verbleiben in der Verantwortung der selbst hostenden Hochschule.

## 16. Verbleibende Risiken im Standardbetrieb

| Risiko | Begrenzung im Standardbetrieb | Lokal zu entscheiden |
| --- | --- | --- |
| Versuchsperson nennt unbeabsichtigt reale oder sensible Angaben | fiktive Aufgabe, klare Verhaltensregel, Korpusprüfung | Umgang mit gefundenen Angaben und gegebenenfalls Ausschluss/Löschung |
| Andere Rolle erkennt Stimme oder zeichnet Kommunikation eigenständig auf | transparente Information, Verbot von Aufzeichnung/Weitergabe, keine Parlando-Rohaufzeichnung | Rekrutierungskontext und Zumutbarkeit |
| Fehlerhaftes Transkript | keine individuelle Bewertung, Forschungszweck, Transkript als maschinell erzeugt erkennbar | wissenschaftliche Qualitätskontrolle |
| Speechmatics verarbeitet Live-Audio | nur bei aktivierter Transkription; kein Roh-Audio in Parlando | Vertrag, Region, Retention und Transfer |
| Reidentifikation aus seltenen Dialogpassagen oder Spielverläufen | struktureller Korpusexport und inhaltliche Prüfung | konkrete Freigabe jedes öffentlichen Korpus |
| Zu lange Aufbewahrung mangels Automatik | manuelle Löschfunktion und vollständige interne Zuordnung | Frist/Kriterium, Zuständigkeit und Nachweis |
| Konfiguration weicht von bewerteter Plattform ab | herunterladbarer Datenschutzstatus und Privacy-Contract-Version | Abweichungsprüfung durch lokalen DSB |

## 17. DSFA-Schwellenbewertung

Der Standardbetrieb richtet sich an Erwachsene, erhebt keine besonderen Kategorien, führt keine biometrische Identifizierung durch, bewertet Personen nicht mit rechtlicher oder ähnlich erheblicher Wirkung und ist nicht auf großflächige Überwachung ausgelegt. Nach unserer Einschätzung begründet die Plattformverwendung innerhalb der beschriebenen Grenzen regelmäßig kein voraussichtlich hohes Risiko im Sinne des Art. 35 DSGVO.

Wir bitten den DSB, diese allgemeine Schwellenbewertung zu bestätigen. Jede einsetzende Hochschule prüft dennoch ihr konkretes Experiment, insbesondere Umfang, Zielgruppe, Sprachverarbeitung, neue Datenverknüpfungen und Abweichungen. Eine einzelne Bewertung kann nach Art. 35 Abs. 1 DSGVO mehrere hinreichend ähnliche Verarbeitungsvorgänge erfassen.

Für UdS-eigene Installationen bleiben das Freigabeverfahren nach § 15 SDSG und das Verzeichnis nach Art. 30 DSGVO zu beachten. Falls für den Plattformbetrieb doch eine DSFA erforderlich ist, ist außerdem § 14 Abs. 2 SDSG einschlägig: Die entwickelnde öffentliche Stelle führt die DSFA für das auch bei anderen öffentlichen Stellen vorgesehene Verfahren durch; bei im Wesentlichen unveränderter Übernahme kann dort eine weitere DSFA unterbleiben. Andere Hochschulen prüfen, ob ihr Landes- und Organisationsrecht eine entsprechende Übernahme vorsieht.

## 18. Kurzes lokales Beiblatt für einsetzende Hochschulen

Eine Hochschule, die unverändertes Parlando selbst hostet, soll für ihre lokale Prüfung nur folgende Angaben ergänzen:

| Angabe | Lokaler Eintrag |
| --- | --- |
| Verantwortliche Hochschule, Forschungseinheit und DSB | [eintragen] |
| Forschungszweck und Rechtsgrundlage | [eintragen] |
| Zielgruppe und Rekrutierung | [eintragen] |
| Parlando- und Privacy-Contract-Version | [Statusbericht beifügen] |
| Speicherschalter | [Statusbericht beifügen] |
| Text, Voice und Speechmatics | [aktiv/inaktiv; Vertrag/Region] |
| Lokales Hosting und Infrastruktur-Auftragsverarbeiter | [eintragen] |
| Aufbewahrung und Zuständigkeit für manuelle Löschung | [eintragen] |
| Nachnutzung und geplante Korpusveröffentlichung | [eintragen] |
| Abweichung vom Standardbetrieb | [nein / beschreiben] |

Liegt keine materielle Abweichung vor, kann die technische Plattformbeschreibung aus dieser Unterlage übernommen werden.

## 19. Änderungen mit erneuter Plattformbewertung

Eine neue Privacy-Contract-Version und erneute Plattformbewertung sind vorgesehen bei:

- neuer dauerhaft gespeicherter Datenart,
- neuer Kategorie externer Empfänger von Teilnehmerdaten,
- Speicherung von Roh-Audio,
- Änderung der festen Forschungs- oder Korpusexporte,
- Änderung der strukturellen Korpusaufbereitung oder der Voraussetzungen für die Anonymitätsentscheidung,
- Änderung der manuellen Teilnehmerlöschung oder
- Änderung des versionierten Informations- und Erklärungsnachweises.

Keine neue Plattformbewertung ist allein erforderlich für:

- ein neues fiktives Spiel innerhalb des Standardbetriebs,
- eine neue Strategie eines vertrauenswürdigen Agenten,
- Änderungen der Benutzeroberfläche ohne neuen Datenfluss,
- Fehlerkorrekturen ohne Änderung des Privacy Contract oder
- das Abschalten einer optionalen Datenart beziehungsweise von Voice oder Transkription.

## 20. Erbetene Form der Stellungnahme

Als Ergebnis erbitten wir eine Stellungnahme mit ungefähr folgender Reichweite:

> Parlando mit Privacy Contract Version 1 wurde für den beschriebenen selbst gehosteten Standardbetrieb technisch und datenschutzrechtlich bewertet. Innerhalb der dokumentierten Grenzen bestehen aus Plattformsicht keine grundsätzlichen datenschutzrechtlichen Einwände. Die Bewertung kann als technische Referenz für hinreichend ähnliche Experimente verwendet werden. Verantwortlicher, Zweck, Rechtsgrundlage, Zielgruppe, Aufbewahrung, lokale Auftragsverarbeiter, Speechmatics-Nutzung und Veröffentlichung eines konkreten Korpus bleiben von der jeweils einsetzenden Hochschule zu prüfen. Wesentliche Abweichungen und Änderungen des Privacy Contract erfordern eine erneute Bewertung.

Der endgültige Wortlaut liegt selbstverständlich im Ermessen des DSB und des Verantwortlichen.

## 21. Inhalt des technischen Abnahmenachweises

Anlage B ist ein versionsbezogener Prüfbericht und kein zusätzliches Konzeptpapier. Sie weist mindestens folgende Punkte nach:

| Prüfpunkt | Im Abnahmenachweis festzuhalten |
| --- | --- |
| Plattformgrenze | geprüfte Version; Bestätigung, dass Experimentmodule nur die beschriebene fiktive Aufgabe, Rollen, Handlungen, Ansichten, Ereignisse und Ergebnisfelder festlegen |
| Teilnehmerablauf | Ergebnis eines Durchlaufs von Information und Erklärung über Raumbeitritt, Rollenzuweisung, Spielhandlung und Kommunikation bis zum Sitzungsabschluss |
| Rollenbezogene Ansichten | Ergebnis der Prüfung, dass verdeckte Rolleninformation serverseitig aus der Ansicht der jeweils anderen Rolle entfernt wird |
| Speicherschalter | Ergebnis je Schalter, einschließlich Nichtvorhandensein deaktivierter Datenarten in Datenbank und Export |
| Informationsnachweis | gespeicherte Version und URL, serverseitiger Hash der konfigurierten Referenz und Consent-Darstellung, archivierte Teilnehmerinformation, Einzelentscheidungen und Zeitpunkt einer Beispielerklärung |
| Datenschutzstatus | erzeugte Anlage A und Prüfung, dass sie mit der wirksamen Konfiguration übereinstimmt und keine Zugangsdaten enthält |
| Exporte | geprüfte Feldstruktur von `research`, `corpus` und `full`; über wiederholte Exporte desselben Experiments konsistente Teilnehmer- und Dialogkennungen sowie Ausschluss interner System-, Rekrutierungs- und Berechtigungskennungen aus `research` und `corpus` |
| Teilnehmerlöschung | Ergebnis von Vorschau, bestätigter Löschung beziehungsweise Ersetzung verbleibender Teilnehmerverweise und anschließendem Export |
| Speechmatics | Bestätigung, dass nur bei aktivierter Transkription Live-Audio übertragen und kein Roh-Audio durch Parlando gespeichert wird |
| ElevenLabs | Bestätigung, dass nur Agententext und technische Voice-/Modellparameter übertragen werden |
| Agenten | Bestätigung, dass Agenten nur die rollenbezogene Spielsicht und die für ihre Rolle bestimmte Kommunikation erhalten |
| Sicherheitsgarantien | Prüfergebnis zu allen in Abschnitt 15 genannten Garantien |

Für jeden Prüfpunkt nennt Anlage B Prüfer, Datum, Methode, Ergebnis und geprüfte Softwareversion beziehungsweise Revision. Reine Verweise auf interne Entwicklungspläne genügen nicht.

## 22. Rechts- und Orientierungstexte

- [Datenschutz-Grundverordnung bei EUR-Lex](https://eur-lex.europa.eu/eli/reg/2016/679/oj?locale=de), insbesondere Art. 5, 6, 9, 13, 24, 25, 28, 30, 32, 35 und 89
- [Saarländisches Datenschutzgesetz](https://recht.saarland.de/bssl/document/jlr-DSGSL2018rahmen), insbesondere §§ 15 und 23
- [Datenschutzbeauftragter der Universität des Saarlandes](https://www.uni-saarland.de/verwaltung/datenschutz.html)
- [Unabhängiges Datenschutzzentrum Saarland: Hinweise für Verantwortliche](https://www.datenschutz.saarland.de/datenschutz/fuer-verantwortliche)
- [EDPB-Leitlinien 07/2020 zu Verantwortlichen und Auftragsverarbeitern](https://www.edpb.europa.eu/our-work-tools/our-documents/guidelines/guidelines-072020-concepts-controller-and-processor-gdpr_de)
- [EDPB Guidelines 1/2026 on processing personal data for scientific research purposes – Konsultationsfassung; Konsultation am 25. Juni 2026 geschlossen](https://www.edpb.europa.eu/public-consultations/guidelines-12026-on-processing-of-personal-data-for-scientific-research_en)
- [EDPB Guidelines 02/2026 on Anonymisation – laufende Konsultationsfassung](https://www.edpb.europa.eu/public-consultations/guidelines-022026-on-anonymisation_en)
- [DSK-Liste der Verarbeitungstätigkeiten mit DSFA-Pflicht](https://www.datenschutz.saarland.de/fileadmin/user_upload/uds/alle_Dateien_und_Ordner_bis_2025/Download/dsfa_muss_liste_dsk_de.pdf)
