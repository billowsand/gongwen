# Symbol table

Canonical crate catalog: [`data/symbols.tsv`](../data/symbols.tsv) (loaded by
`latex_rust::symbols`). Milestone 5 golds require every row to parse, and every
bare single-glyph Symbol/Operator row to layout and SVG-render as that Unicode
character with the correct TeX atom class.

The crate catalog is the renderer contract.

Dedup: one duplicate `\sqrt{}` row was dropped. `\sqrt[{}]{}` is kept as the
nth-root form.
