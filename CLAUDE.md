# xpath-eval

Rust-Crate: eigenständiger XPath-1.0-Ausdrucks-Parser und -Evaluator.
Konzept & Herkunft: `README.md`. Umsetzungsplan: `plan/`.

Kein eigenes Projekt-Repo-Umbenennungsrisiko wie bei `relax-ng`: Projektname
(Repo) und voraussichtlicher crates.io-Paketname sind hier identisch,
`xpath-eval`. Das ist keine Garantie — vor Veröffentlichung erneut prüfen.

Schwesterprojekt von [`schematron-engine`](../schematron-engine) (nutzt
dieses Crate für `test="..."`-Auswertung) und von
[`html-conform`](../html-conform) (künftiger Ersatz für `xmloxide`s
XPath/Schematron-Nutzung in dessen Assertion-Schicht, Phase 06). Steht aber
für sich: generisch, keine HTML-/Schematron-/sonstige Format-Abhängigkeit.

## Architektur (Arbeitstitel, siehe `plan/` für Details)

```
XPath-1.0-Ausdruck (String) → Lexer/Parser → AST
                             → Auswertung gegen ein generisches Document/Node-Trait
                             → XPath-Wert (Node-Set / String / Number / Boolean)
```

Wer einen Baum auswerten will, bringt sein eigenes Dokument mit (über ein
Trait) — dieses Crate parst/baut keine XML- oder HTML-Bäume selbst.

## Arbeitsweise

- Aktueller Stand & nächster Schritt: `plan/00-STATUS.md`.
- Phasenpläne mit Schritten/Exit-Kriterien: `plan/0N-*.md`. Vor größeren
  Änderungen die passende Phase lesen, nicht am Plan vorbei arbeiten.
- Getroffene Entscheidungen: `plan/DECISIONS.md` — dort nachschlagen,
  bevor offene Fragen neu aufgerollt werden.

## Feste Regeln

- Lizenz: **MIT**, von Anfang an (`Cargo.toml`: `license = "MIT"`).
- Normative Grundlage ist die [XPath-1.0-Spezifikation](https://www.w3.org/TR/1999/REC-xpath-19991116/).
  Bei Unklarheiten dort nachschlagen, nicht aus anderen Implementierungen
  raten.
- Kein HTML-, XML-Parser- oder sonstige Host-Format-Abhängigkeit im Kern —
  Instanzdokumente kommen ausschließlich über ein generisches Trait rein.
- Kein `unsafe` ohne expliziten Grund und Kommentar.

## Definition of Done

Siehe "Exit-Kriterien" in der jeweiligen `plan/0N-*.md`-Datei — nicht
global definiert, sondern pro Phase.
