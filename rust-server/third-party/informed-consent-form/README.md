# Informed consent template attribution

Source: https://github.com/Wuhn/informed-consent-form/tree/main/template
License: Apache-2.0 (included in LICENSE).

The Markdown templates in `templates/informed-consent/` supply the dashboard’s
informed-consent section headings and descriptions adapt the structure of
`study_informed_consent_1.html` (blob acbe64d503efd21e9cd72f48ee8885ca420c4e36) and
`study_informed_consent_2.html` (blob a7cf87e98eebe57644d04e13579fbe5c06ca60e4).

Modifications: organized the material into fourteen selectable sections and brief
purpose descriptions, splitting collected information into personal and non-personal
sections and removing the introductory reading instruction. Section bodies start with the previously adapted template prose, including explicit
placeholders instead of institution-specific legal, anonymity, risk, and withdrawal
claims. Study conductors edit that prose before generation. Selected headings
and conductor text are combined into one editable consent statement with one
acceptance requirement in Parlando’s existing recorded-consent flow. The Flask
navigation and GET consent form are replaced by Parlando’s authenticated consent
submission and required-consent admission checks.
