# Word, PowerPoint and Excel to PDF

> **Status:** M10 candidate, native import of Office Open XML · **Revision history:** 2026-10-14 first version; 2026-10-16 #2871 workflow completion and reporting; 2026-10-22 Turkish and Central European letters drawn, UI notices translated and shown only when something was approximated; 2026-10-25 review fixes (Word 2010 text boxes, nested boxes, master placeholders, merged table cells, stored column widths) and the known gaps listed; 2026-10-27 wording: what can be left out, which workflows convert off the UI thread; 2026-10-28 right, centre and decimal tabs with leaders, plain table-of-contents links, paragraph borders, table-style parts, cell vertical alignment, header and footer tables, page-anchored floating tables, EMF pictures as vectors; Excel headers and footers, print titles, print areas, centring, manual breaks, page order, pictures and shapes on sheets, conditional formatting.

PdfCraft opens Word (`.docx`, `.docm`, `.dotx`), PowerPoint (`.pptx`, `.pptm`, `.ppsx`, `.potx`) and
Excel (`.xlsx`, `.xlsm`, `.xltx`) files as new, unsaved PDFs. The conversion is native Rust in
`pdfcraft-create` (`from_office`): no office suite, converter process or network is involved, and
it works in the browser build too. The source file is only read.

## Where it is reached

- **File ▸ Open** (and Create ▸ PDF from File) has a *Word, PowerPoint and Excel* filter. The open
  worker converts in the background and the result opens in a new tab; Save As chooses where it
  goes. If something was approximated, a notice says what.
- **Create PDF from multiple files**, **Combine** and **batch convert** accept Office files the
  same way they accept images and text (`Session::convert_to_pdf`). Create PDF from multiple
  files and Combine say what each conversion approximated (`Session::convert_to_pdf_reported`).
- **Automation:** `doc_create` with `{"from": "office", "path": "report.docx"}` returns the new
  document like `doc_open`, plus an `office` object (`kind`, `pages`, `escaped`, `notes`).
  `--root` confines the path as for every other tool.
- **Direct automation open:** `doc_open` converts Office inputs into unsaved PDFs with no source
  save path. It returns the same `office` report as `doc_create`.
- **Organize Pages insertion:** `page_insert_file` accepts converted Office/image/text inputs as
  the UI does, keeps 1-based source page selection, and remains undoable. Office conversion returns
  an `office` report; a failed conversion leaves the destination unchanged.
- **Conversion notices (#2871 follow-up):** Open (native and in-memory) and Insert say, in the
  interface language, what was approximated; an exact conversion opens without a notice. Combine
  puts the same sentence in the row's warnings. Automation results carry an English `notice` that
  also states the standard-font caveat, and `doc_combine` returns a `conversions` array with
  per-source paths, approximation counts and notices. Deep content skipped at the nesting
  limit is counted. Source files are never written by these operations.

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
  spacing (space before kept at the top of a page a break started, dropped after a full page), left,
  right, centre and decimal tab stops with dot, hyphen, underscore and middle-dot leaders,
  paragraph borders (boxes shared by consecutive paragraphs, double rules) and shading, numbered
  and bulleted lists (`numbering.xml`, nine levels, restarts), tables (column grid, spans,
  vertical merges, cell shading, table-style and cell borders, the table style's header, total
  and banded rows and first and last columns (`tblLook`), cell vertical alignment, rows split
  across pages, tables floating against the page or its margins), inline and anchored pictures
  (PNG, JPEG, and EMF drawn as vectors), page breaks, headers and footers per section
  (first-page variants, tables laid out as tables) with live `PAGE` and `NUMPAGES` fields,
  table-of-contents links printed without the Hyperlink style (as Word prints them), footnotes
  and endnotes (placed at the end with their marks), equations as plain text, and text boxes'
  text.
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
  borders, horizontal and vertical alignment, indents, wrapped text (across a merged range's whole
  width), merged cells, overflow into empty neighbours and `###` for numbers that don't fit.
  Formulas show their saved results. Print setup: headers and footers (`&L &C &R`, `&P &N &D &T
  &F &A`, fonts, sizes, colours, bold/italic/underline, first-page and odd/even variants, `&G`
  pictures from `legacyDrawingHF`), print areas (each on its own pages), print title rows and
  columns, horizontal and vertical centring, manual row and column breaks, over-then-down page
  order. Pictures (PNG, JPEG, EMF) and shapes (rectangles, rounded rectangles, ellipses, arrows,
  triangles, diamonds, lines with arrowheads, with fill, outline and text) on sheets, at their
  anchors. Conditional formatting: cell value, top/bottom N and N %, above/below average,
  duplicate and unique values, text, blanks and errors, time periods, colour scales, data bars and
  icon sets, by priority with stop-if-true.

## What is approximated

Content-specific approximations are counted in the report (`notes`); automation's `notice` also
states the standard-font substitution. Nothing in the source is changed.

- **Fonts:** text is set in the standard PDF faces (Helvetica, Times, Courier and their bold and
  italic styles), chosen from each run's font family: Calibri, Arial, Aptos → Helvetica;
  Cambria, Georgia, Times New Roman → Times; Consolas, Courier New → Courier. No font program is
  embedded, so the PDF stays small and searchable, but line breaks can differ from Office's.
  Beyond WinAnsi, the Turkish, Central European, Baltic and Romanian letters (ğ ı İ ş, ą ć ę ł ń
  ś ź ż, č ř ů, ő ű, ā ē ī ū, ș ț…) are drawn from the same faces through a second encoding with a
  `/ToUnicode` map, so they look like the surrounding text and stay searchable. `ď ľ Ľ ť ģ Đ đ`
  are not among them: viewers that stand in for the standard faces (PdfCraft's renderer included)
  build accented letters from a base letter and a plain accent and can't build these. They, and
  everything the faces can't draw at all (Greek, Cyrillic, CJK, Arabic, emoji…), are written as
  `[U+XXXX]` and counted; they are never replaced by `?`. Embedding real faces would cover them.
- **Not drawn, shown as outlined boxes:** charts, SmartArt, embedded objects without an EMF or
  bitmap preview, and pictures in a format PdfCraft can't decode (WMF, SVG) or linked rather than
  embedded. EMF pictures are played as vectors (pens, brushes, fonts, rectangles, rounded
  rectangles, ellipses, polygons, polylines, Béziers, paths, text, mapping modes and transforms);
  their bitmaps, clipping regions and arcs are skipped.
- **Simplified:** gradients use their middle colour; arcs in custom shapes are drawn straight;
  preset shapes without a specific outline use their bounding rectangle; vertical text is drawn
  horizontally; text boxes in Word join the text flow after their paragraph; multi-column
  sections are set in one column; Excel chart sheets are left out; `&D`/`&T` print the date and
  time in UTC as `yyyy-mm-dd` and `h:mm AM/PM`; pages are numbered through the whole workbook (a
  sheet's own first page number is not used, as when Excel prints a workbook); printed row and
  column headings are not drawn.
- **Known gaps, not yet handled** (the source file is never changed, but some content is
  simplified or left out of the PDF, and the conversion report notes the content-specific cases):
  - Word: a picture's crop (`srcRect`) is ignored, so a cropped picture is squeezed; paragraph
    borders and shading inside table cells are not drawn; tables floating in the text
    (`vertAnchor="text"`) stay in the flow without text wrapping around them; `STYLEREF`,
    `DATE` and `PRINTDATE` keep their saved results; dotted, dashed and wavy borders are drawn
    solid.
  - PowerPoint: transparency (`alpha`) is ignored, so a translucent overlay is drawn opaque;
    curved and bent connectors are drawn as boxes; Wingdings bullets show as the letters they are
    stored as; text in a flipped shape is flipped with it; placeholders inherit no fill or
    outline from the layout or master.
  - Excel: a number that just fits its column in Calibri can be shown as `####`, as Helvetica is
    wider; General numbers aren't shortened to fit; outside borders of merged ranges come from
    the first cell only; rows without a stated height aren't grown for large fonts; ISO date
    cells (`t="d"`) are left empty; conditional formatting rules that need a formula
    (`expression`) are left out and reported; charts on sheets are outlined boxes; grouped
    shapes are left out.
  - Desktop app: Create PDF from multiple files converts each file on the UI thread (as it does
    images and text), so a large Office file pauses the window while it converts; File ▸ Open
    converts off the UI thread.
- **Not supported:** password-protected files and the legacy binary formats (`.doc`, `.ppt`,
  `.xls`) — they are refused with a message saying so — and OpenDocument files.

## Safety

Packages are untrusted input. The zip reader accepts only stored or deflated parts, refuses zip64
and encryption, caps each part at 128 MiB and a package at 512 MiB decompressed, and never
inflates past a part's declared size, counting what a failed part inflated. A sheet keeps at most
64 MiB of cell text (the rest is left out and reported). XML is parsed with DTDs disallowed and a node limit. Style
chains, nesting, table spans, counts and every number read from the file are bounded, so a
hostile file is refused or approximated, never a crash. Conversion runs under the engine's panic
guard, and in File ▸ Open it runs off the UI thread. Macros (`vbaProject.bin`) are never read.

## Verification

`crates/create/src/office/tests.rs` builds Word, PowerPoint and Excel packages from synthetic text
and pixels in the test itself (no committed fixture) and checks the reopened PDF: page sizes,
fonts, text, list labels, table cells, images, header page numbers, inherited placeholder styles,
theme colours, autofit, number formats, merged cells, hidden content, tab stops and leaders,
table-of-contents links, paragraph borders, table-style parts, header tables, floating tables,
EMF pictures, Excel headers and footers, print titles and areas, conditional formatting, sheet
pictures and shapes, and hostile input (`emf.rs`, `xlsx_hf.rs`, `xlsx_cf.rs` have their own unit
tests, hostile metafiles included).
`crates/automation/tests/it/automation.rs::creating_pdfs_from_word_and_powerpoint_natively` exports a
PDF to `.docx` and `.pptx` with PdfCraft's own exporter and imports them back through `doc_create`.
