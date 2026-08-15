# Umgesetzte Datenschutz-Roadmap für Parlando

**Stand:** 14. August 2026<br>
**Status:** Die vereinbarten sechs Änderungen sind umgesetzt.<br>
**Bezug:** [`datenschutz-pruefvorlage.md`](datenschutz-pruefvorlage.md) und die maßgeblichen [`security-ground-rules.md`](security-ground-rules.md)

## Ziel und Grenzen

Parlando unterstützt freiwillige Experimente, in denen zwei Erwachsene oder ein Mensch und ein vertrauenswürdiger Software-Agent eine fiktive Aufgabe bearbeiten. Typische Forschungsdaten sind Spielhandlungen, technische Zeitpunkte, Ergebnisse und – abhängig von der Konfiguration – Tastatur-Chat oder finale Sprachtranskripte. Parlando fragt keinen Teilnehmernamen ab und ist nicht für die gezielte Erhebung besonderer Kategorien personenbezogener Daten ausgelegt.

Wissenschaftliche Nachnutzung und die spätere Veröffentlichung anonymisierter Spiel- und Dialogkorpora gehören zum normalen Einsatz. Parlando erzeugt dafür pseudonymisierte Forschungsdaten und einen veröffentlichungsorientierten Korpuskandidaten. Ob ein konkretes Korpus nach Entfernung der Zuordnungen und Inhaltsprüfung tatsächlich anonym ist, entscheidet die verantwortliche Forschungseinrichtung vor der Veröffentlichung.

Remote-Agenten sind kontrollierter Experimentcode. Ein fremdbetriebener Agenten- oder Modelldienst, der Teilnehmerdaten außerhalb der Kontrolle der verantwortlichen Hochschule verarbeitet, liegt außerhalb dieses Standardbetriebs und benötigt eine eigene Bewertung.

Die Sicherheitsgrenzen für Administration, Teilnehmerauthentisierung, Origins, Ressourcen, Persistenz, Secrets, Container und Remote-Agenten sind im Sicherheitsnachweis beschrieben und im Code umgesetzt. Lokale Release- und Betriebsprüfungen bleiben Aufgabe der betreibenden Hochschule.

## 1. Versionierte Teilnehmerinformation und Erklärungsnachweis

Jedes Experiment kann eine feste Versionskennung und URL seiner Teilnehmerinformation konfigurieren. Parlando berechnet serverseitig einen Hash aus dieser Versionskennung, der URL und der geordneten Darstellung aller konfigurierten Consent-Items. Mit jeder Erklärung speichert der Server:

- Experiment und Teilnehmer,
- Versionskennung, Informations-URL und Hash der konfigurierten Darstellung,
- Entscheidung zu jedem konfigurierten Consent-Item und
- Zeitpunkt der Erklärung.

Unbekannte Consent-Items werden abgelehnt. Alle erforderlichen Erklärungen müssen erfolgreich gespeichert sein, bevor ein Teilnehmer einen Raum betreten kann. Der Hash belegt die serverseitig konfigurierte Referenz und Consent-Darstellung, nicht den unabhängig gehosteten Seiteninhalt hinter der URL. Deshalb wird die veröffentlichte Teilnehmerinformation zusätzlich unter ihrer Versionskennung archiviert. Eine materielle Textänderung erhält eine neue lokale Versionskennung und neue Consent-Item-IDs.

## 2. Kennungen für Menschen, Agenten und Dialoge

Parlando erzeugt bei der Anlage dreiteilige, menschenlesbare Zufallskennungen ausschließlich für menschliche Teilnehmer und Dialoge. Menschliche Teilnehmerkennungen enden in einem Tiernamen, Dialogkennungen in einem Orts- oder Objektnamen. Die getrennten Wortlisten machen die Kennungsart ohne Präfix erkennbar.

Agententeilnehmer erhalten keinen Zufallsnamen. Ihre Kennung nennt stattdessen Agententyp, Implementierungsname, soweit vorhanden, und Version, beispielsweise `agent:space_game.back_and_forth:BackAndForthAgent@0.2.0`. Fehlt eine Versionsangabe, wird dies mit `unversioned` ausdrücklich sichtbar gemacht. Damit lassen sich Beiträge unterschiedlicher Agentimplementierungen in Adminansicht und Export unmittelbar unterscheiden.

Eine menschliche Teilnehmerkennung gilt nur innerhalb eines Experiments. Dieselbe externe Rekrutierungskennung erhält in einem anderen Experiment einen unabhängig erzeugten Namen. Innerhalb desselben Experiments bleiben menschliche Teilnehmer-, Agenten- und Dialogkennungen über Sitzungen und wiederholte Exporte unverändert.

Solange eine Rekrutierungszuordnung oder ein anderes realistisches Zuordnungsmittel besteht, sind die Forschungsdaten pseudonymisierte personenbezogene Daten. Nach Entfernung dieser Zuordnung und erfolgreicher Inhaltsprüfung können die zufälligen Kennungen als nicht personenbezogene Korpuslabels erhalten bleiben.

Parlando besitzt kein Eingabefeld und keine Datenbankspalte für Teilnehmer-Anzeigenamen. Der normale Forschungs- und Korpusexport enthält keine externe Rekrutierungskennung, keinen öffentlichen Teilnehmer-Session-Handle und keine Berechtigungsnachweise.

Sprachdiagnosen sind auf feste Ereignisnamen und wenige skalare Transportmesswerte begrenzt. Mikrofon-Geräte-ID, Gerätebezeichnung, vollständiger User-Agent und freie Browserfehlertexte werden nicht gespeichert.

## 3. Vier Speicherschalter

Die wirksame Experimentkonfiguration enthält vier Schalter:

```yaml
privacy:
  store_full_game_state: true
  store_typed_messages: true
  store_final_transcripts: true
  store_voice_diagnostics: false
```

Die Schalter steuern, ob die jeweilige Datenart dauerhaft in SQLite geschrieben wird. Sie ändern weder die bestehende Nachrichtenstruktur noch das Ereignisschema.

- `store_full_game_state` steuert vollständige Zustandsabbilder in Ereignissen.
- `store_typed_messages` steuert die Speicherung menschlicher Tastatur-Chatnachrichten mit Ursprung `typed`; die Live-Übermittlung bleibt erhalten.
- `store_final_transcripts` steuert die Speicherung finaler Transkripte und daraus erzeugter Konversationsnachrichten.
- `store_voice_diagnostics` steuert minimierte technische Sprachdiagnosen.

Deaktivierte Datenarten erscheinen weder in SQLite noch in daraus erzeugten Exporten.

## 4. Feste Exportvarianten

Parlando stellt drei feste, serverseitige Exportvarianten bereit:

- `research`: pseudonymisierte Forschungsdaten mit experimentbezogenen Teilnehmer- und Dialogkennungen, aktivierten Spiel-, Nachrichten-, Transkript- und Ergebnisdaten sowie wissenschaftlich benötigten Zeitangaben;
- `corpus`: veröffentlichungsorientierter `corpus_candidate` mit denselben experimentbezogenen Zufallskennungen, ohne interne Systemkennungen und mit relativen statt absoluten Zeitangaben;
- `full`: vollständiger interner Datensatz für eng begrenzte Administration und Betroffenenanfragen.

`research` und `corpus` verwenden feste Feld-Allowlisten. Neue interne Datenbankfelder gelangen dadurch nicht automatisch in diese Exporte. Rekrutierungskennungen, Teilnehmer-Session-Handles, Erklärungsnachweise, vollständige Konfiguration und Zugangsdaten sind ausgeschlossen.

Der `corpus`-Export ist noch kein Nachweis der Anonymität. Freie Dialoginhalte und seltene Spielverläufe können identifizierende Angaben enthalten. Vor einer öffentlichen Freigabe entfernt die verantwortliche Stelle bestehende Zuordnungen, prüft das Korpus inhaltlich und dokumentiert ihre Anonymitätsentscheidung. Ist eine Identifizierung weiterhin mit realistisch verfügbaren Mitteln möglich, bleiben die Daten unter kontrolliertem Zugang.

## 5. Manuelle Teilnehmerlöschung

Im Admin-Webinterface steht für menschliche Teilnehmer „Delete participant data“ zur Verfügung. Eine Vorschau zeigt die experimentbezogene Teilnehmerkennung, betroffene Sitzungen und die Anzahl der betroffenen Erklärungs-, Kommunikations- und Ereignisdatensätze. Nach Bestätigung:

- werden externe Rekrutierungskennung, Teilnehmerkennung und Teilnehmermetadaten entfernt,
- werden Erklärungsdatensätze sowie verfasste Nachrichten und Transkripte gelöscht,
- werden Teilnehmer-Session-Handles ersetzt und
- werden verbleibende gemeinsam benötigte Spielereignisse auf `deleted_participant` zurückgeführt.

Damit bleiben der fiktive gemeinsame Spielverlauf und die Daten der anderen Rolle nutzbar, ohne dass Parlando darin eine rückführbare Teilnehmerkennung behält. Die Funktion ist manuell; Parlando führt bewusst keinen automatischen Aufbewahrungs- oder Löschjob aus.

## 6. Datenschutzstatus im Adminbereich

Die geschützte Route `/admin/privacy` zeigt installationsweite technische Tatsachen aus Serverversion und wirksamer Konfiguration. Sie nennt:

- Parlando-Version, Git-Revision und `privacy_contract_version`,
- aktive Speicherschalter und gespeicherte Datenarten,
- konfigurierte externe Sprachdienste und deren Datenfluss,
- Nicht-Speicherung von Roh-Audio,
- Exportvarianten,
- experimentbezogene Kennungslogik,
- manuelle Teilnehmerlöschung und
- versionierten Informations- und Erklärungsnachweis.

Der Status kann als Markdown oder JSON heruntergeladen werden und enthält keine Secrets. Er behauptet keine organisatorisch nicht erkennbaren Tatsachen wie Verantwortlicher, Selbsthosting, Rechtsgrundlage, Aufbewahrung oder Vertragsstatus. Diese Angaben ergänzt die betreibende Hochschule in ihrem lokalen Beiblatt.

## Sprachdienste

- Bei aktivierter Transkription erhält Speechmatics Live-Mikrofon-Audio; Parlando speichert kein Roh-Audio.
- ElevenLabs erhält ausschließlich den vom Software-Agenten erzeugten Text und technische Voice-/Modellparameter. Mikrofon-Audio, Teilnehmernachrichten, Transkripte, Kennungen und Spielzustände werden nicht an ElevenLabs gesendet.
- Lokale und abgesicherte Remote-Agenten erhalten nur die rollenbezogene Spielsicht und die für ihre Rolle bestimmte Kommunikation.

## Bewusst nicht implementiert

Parlando enthält kein allgemeines Datenschutz-Managementsystem, keine frei konfigurierbare Export-Policy, keine teilnehmerbezogene Zweckmatrix und keinen automatischen Löschplan. Diese Funktionen sind für den beschriebenen Standardbetrieb nicht erforderlich. Neue Datenschutzmechanismen werden erst ergänzt, wenn ein konkretes Experiment sie benötigt und der Privacy Contract entsprechend neu bewertet wird.
