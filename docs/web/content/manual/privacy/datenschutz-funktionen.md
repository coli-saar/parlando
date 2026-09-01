---
title: Technische Datenschutzfunktionen von Parlando
---

# Technische Datenschutzfunktionen von Parlando

Dieses Dokument beschreibt die implementierten Datenschutzfunktionen des Plattformkerns. Es ergänzt
die [`datenschutz-pruefvorlage.md`](datenschutz-pruefvorlage/). Organisatorische Entscheidungen der
betreibenden Einrichtung, insbesondere Rechtsgrundlage, Aufbewahrung und Freigabe, bleiben außerhalb
des Plattformkerns.

## Ziel und Grenzen

Parlando unterstützt freiwillige Experimente, in denen zwei Erwachsene oder ein Mensch und ein vertrauenswürdiger Software-Agent eine fiktive Aufgabe bearbeiten. Typische Forschungsdaten sind Spielhandlungen, technische Zeitpunkte, Ergebnisse und – abhängig von der Konfiguration – Tastatur-Chat oder finale Sprachtranskripte. Parlando fragt keinen Teilnehmernamen ab und ist nicht für die gezielte Erhebung besonderer Kategorien personenbezogener Daten ausgelegt.

Wissenschaftliche Nachnutzung und die spätere Veröffentlichung anonymisierter Spiel- und Dialogkorpora gehören zum normalen Einsatz. Parlando erzeugt dafür pseudonymisierte Forschungsdaten und einen veröffentlichungsorientierten Korpuskandidaten. Ob ein konkretes Korpus nach Entfernung der Zuordnungen und Inhaltsprüfung tatsächlich anonym ist, entscheidet die verantwortliche Forschungseinrichtung vor der Veröffentlichung.

Remote-Agenten sind kontrollierter Experimentcode. Ein fremdbetriebener Agenten- oder Modelldienst, der Teilnehmerdaten außerhalb der Kontrolle der verantwortlichen Hochschule verarbeitet, liegt außerhalb dieses Standardbetriebs und benötigt eine eigene Bewertung.

Die Sicherheitsgrenzen für Administration, Teilnehmerauthentisierung, Origins, Ressourcen, Persistenz, Secrets, Container und Remote-Agenten sind im Sicherheitsnachweis beschrieben und im Code umgesetzt. Lokale Release- und Betriebsprüfungen bleiben Aufgabe der betreibenden Hochschule.

## 1. Versionierte Teilnehmerinformation und Erklärungsnachweis

Jedes Experiment kann eine feste Versionskennung und URL seiner Teilnehmerinformation konfigurieren. Mit jeder Erklärung speichert Parlando:

- Experiment und Teilnehmer,
- Versionskennung und Informations-URL,
- Entscheidung zu jedem konfigurierten Consent-Item und
- Zeitpunkt der Erklärung.

Alle erforderlichen Erklärungen müssen erfolgreich gespeichert sein, bevor ein Teilnehmer eintreten kann. Parlando archiviert den Inhalt der verlinkten Informationsseite nicht selbst. Deshalb archiviert die verantwortliche Stelle die tatsächlich veröffentlichte Fassung unter ihrer Versionskennung. Eine materielle Textänderung erhält eine neue lokale Versionskennung und neue Consent-Item-IDs.

## 2. Kennungen für Menschen, Agenten und Dialoge

Parlando erzeugt menschenlesbare Zufallskennungen für menschliche Teilnehmer und Dialoge. Agenten werden stattdessen durch ihre konfigurierte Implementierung und Version bezeichnet, damit ihre Beiträge in Adminansicht und Export unterscheidbar bleiben.

Eine menschliche Teilnehmerkennung gilt nur innerhalb eines Experiments. Dieselbe externe Rekrutierungskennung erhält in einem anderen Experiment einen unabhängig erzeugten Namen. Innerhalb desselben Experiments bleiben menschliche Teilnehmer-, Agenten- und Dialogkennungen über Sitzungen und wiederholte Exporte unverändert.

Solange eine Rekrutierungszuordnung oder ein anderes realistisches Zuordnungsmittel besteht, sind die Forschungsdaten pseudonymisierte personenbezogene Daten. Nach Entfernung dieser Zuordnung und erfolgreicher Inhaltsprüfung können die zufälligen Kennungen als nicht personenbezogene Korpuslabels erhalten bleiben.

Parlando fragt keinen Teilnehmer-Anzeigenamen ab. Der normale Forschungs- und Korpusexport enthält keine externe Rekrutierungskennung und keine Zugangsdaten.

Mikrofon-Geräte-ID, Gerätebezeichnung, vollständiger User-Agent und freie Browserfehlertexte werden nicht gespeichert.

## 3. Einheitliche Erhebung

Parlando speichert akzeptierte Handlungen, resultierende Zustände und Abschlüsse. Tastaturkommunikation und finale Transkripte werden gespeichert, wenn die jeweilige Kommunikationsform verwendet wird. Roh-Audio wird nicht dauerhaft gespeichert.

## 4. Feste Exportvariante

Parlando stellt einen festen Korpuskandidaten bereit. Er enthält das Experiment, die Nicht-Testsitzungen, Rollen, Abschlüsse sowie die für die Forschung vorgesehenen Handlungen und Nachrichten. Rekrutierungskennungen, Erklärungsnachweise, administrative Daten und Zugangsdaten sind ausgeschlossen.

Der `corpus`-Export ist noch kein Nachweis der Anonymität. Freie Dialoginhalte und seltene Spielverläufe können identifizierende Angaben enthalten. Vor einer öffentlichen Freigabe entfernt die verantwortliche Stelle bestehende Zuordnungen, prüft das Korpus inhaltlich und dokumentiert ihre Anonymitätsentscheidung. Ist eine Identifizierung weiterhin mit realistisch verfügbaren Mitteln möglich, bleiben die Daten unter kontrolliertem Zugang.

## 5. Manuelle Teilnehmerlöschung

Im Admin-Webinterface steht für menschliche Teilnehmer „Delete participant data“ zur Verfügung. Eine Vorschau zeigt die experimentbezogene Teilnehmerkennung, betroffene Sitzungen und die Anzahl der betroffenen Erklärungs-, Kommunikations- und Ereignisdatensätze. Nach Bestätigung:

- werden alle betroffenen gemeinsamen Sitzungen mit ihren Ereignissen, Rollenzuordnungen und sitzungsbezogenen Erklärungen gelöscht und
- wird anschließend der Teilnehmerdatensatz mit Rekrutierungszuordnung, Teilnehmerkennung, Metadaten und verbleibenden Erklärungen physisch gelöscht.

Damit wird auch der Beitrag der anderen Rolle in jeder gemeinsamen Sitzung gelöscht. Laufende oder noch nicht abgeschlossene Sitzungen blockieren die Löschung, damit keine späteren Runtime-Schreibvorgänge Daten wieder anlegen.

## 6. Datenschutzstatus im Adminbereich

Die geschützte Route `/admin/privacy` zeigt installationsweite technische Tatsachen aus Serverversion und wirksamer Konfiguration. Sie nennt:

- Parlando-Version, Git-Revision und `privacy_contract_version`,
- tatsächlich gezählte gespeicherte Datenarten,
- konfigurierte externe Sprachdienste und deren Datenfluss,
- Nicht-Speicherung von Roh-Audio,
- Inhalt und Grenzen des Korpuskandidaten,
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
