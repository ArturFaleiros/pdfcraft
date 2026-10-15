# Word, PowerPoint and Excel to PDF

> **Status:** M10 candidate, native import of Office Open XML · **Revision history:** 2026-10-14 first version.

PdfCraft opens Word (`.docx`, `.docm`, `.dotx`), PowerPoint (`.pptx`, `.pptm`, `.ppsx`, `.potx`) and
Excel (`.xlsx`, `.xlsm`, `.xltx`) files as new, unsaved PDFs. The conversion is native Rust in
`pdfcraft-create` (`from_office`): no office suite, converter process or network is involved, and
it works in the browser build too. The source file is only read.

## Where it is reached

- **File ▸ Open** (and Create ▸ PDF from File) has a *Word, PowerPoint and Excel* filter. The open
  worker converts in the background and the result opens in a new tab; Save As chooses where it
  goes. If something was approximated, a notice says what.
- **Create PDF from multiple files**, **Combine** and **batch convert** accept Office files the
  same way they accept images and text (`Session::convert_to_pdf`).
- **Automation:** `doc_create` with `{"from": "office", "path": "report.docx"}` returns the new
  document like `doc_open`, plus an `office` object (`kind`, `pages`, `escaped`, `notes`).
  `--root` confines the path as for every other tool.

```json
[
  {"tool": "doc_create", "args": {"from": "office", "path": "report.docx"}},
  {"tool": "doc_save", "args": {"doc": 1, "path": "report.pdf"}}
]
```

## What is carried over

- **Word:** sections (page size, orientation, margins), paragraph and character styles with their
  `basedOn` chains and document defaults, direct formatting (font family, size, bold, italic,
  underline, strike-through, colour, highlight and shading, super- and subscript, caps, hidden
  text), alignment including justification, indents and hanging indents, spacing and line
  spacing, tab stops, numbered and bulleted lists (`numbering.xml`, nine levels, restarts),
  tables (column grid, spans, vertical merges, cell shading, table-style and cell borders, rows
  split across pages), inline and anchored pictures, page breaks, headers and footers per section
  (first-page variants) with live `PAGE` and `NUMPAGES` fields, footnotes and endnotes (placed at
  the end with their marks), equations as plain text, and text boxes' text.
- **PowerPoint:** one page per visible slide at the slide size; backgrounds (solid, gradient as one
  colour, picture); master and layout decorations; placeholder geometry and text styles inherited
  from the layout and master (`titleStyle`, `bodyStyle`, list styles); theme colours with
  `lumMod`/`lumOff`/`tint`/`shade`, theme fonts; bullets (characters and automatic numbering);
  `normAutofit` scaling; text anchoring and insets; shapes with fills, outlines, rotation and
  flips (common preset geometries and custom paths); pictures with cropping; groups; connectors;
  tables (with a default banded style); slide-number fields. Hidden slides are left out, as
  PowerPoint does when saving a PDF.
- **Excel:** each visible sheet's used range at 100 %, column widths and row heights (hidden ones
  skipped), pages ordered down then across, gridlines when the sheet asks for them; cell values
  with their number formats (General, decimals and thousands, percentages, scientific, currency
  tags such as `[$R$-416]` with the locale's decimal comma, dates and times with English or
  Portuguese month and day names, sections for negatives and zero, `[Red]`); fonts, fills,
  borders, horizontal and vertical alignment, indents, wrapped text, merged cells, overflow into
  empty neighbours and `###` for numbers that don't fit. Formulas show their saved results.

## What is approximated

Every approximation is counted in the report (`notes`), and nothing in the source is changed.

- **Fonts:** text is set in the standard PDF faces (Helvetica, Times, Courier and their bold and
  italic styles), chosen from each run's font family: Calibri, Arial, Aptos → Helvetica;
  Cambria, Georgia, Times New Roman → Times; Consolas, Courier New → Courier. No font program is
  embedded, so the PDF stays small and searchable, but line breaks can differ from Office's.
  Characters those faces can't draw (beyond Western European: CJK, Arabic, emoji…) are written as
  `[U+XXXX]` and counted; they are never replaced by `?`.
- **Not drawn, shown as outlined boxes:** charts, SmartArt, embedded objects, and pictures in a
  format PdfCraft can't decode (EMF, WMF, SVG) or linked rather than embedded.
- **Simplified:** gradients use their middle colour; arcs in custom shapes are drawn straight;
  preset shapes without a specific outline use their bounding rectangle; vertical text is drawn
  horizontally; text boxes in Word join the text flow after their paragraph; multi-column
  sections are set in one column; Excel charts and pictures, chart sheets, headers and footers,
  and print areas are not used.
- **Not supported:** password-protected files and the legacy binary formats (`.doc`, `.ppt`,
  `.xls`) — they are refused with a message saying so — and OpenDocument files.

## Safety

Packages are untrusted input. The zip reader accepts only stored or deflated parts, refuses zip64
and encryption, caps each part at 128 MiB and a package at 512 MiB decompressed, and never
inflates past a part's declared size. XML is parsed with DTDs disallowed and a node limit. Style
chains, nesting, table spans, counts and every number read from the file are bounded, so a
hostile file is refused or approximated, never a crash. Conversion runs under the engine's panic
guard and, in the desktop app, off the UI thread. Macros (`vbaProject.bin`) are never read.

## Verification

`crates/create/src/office/tests.rs` builds Word, PowerPoint and Excel packages from synthetic text
and pixels in the test itself (no committed fixture) and checks the reopened PDF: page sizes,
fonts, text, list labels, table cells, images, header page numbers, inherited placeholder styles,
theme colours, autofit, number formats, merged cells, hidden content, and hostile input.
`crates/automation/tests/automation.rs::creating_pdfs_from_word_and_powerpoint_natively` exports a
PDF to `.docx` and `.pptx` with PdfCraft's own exporter and imports them back through `doc_create`.
