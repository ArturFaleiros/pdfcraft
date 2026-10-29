# Word, PowerPoint and Excel to PDF

> **Status:** M10 candidate, native import of Office Open XML · **Revision history:** 2026-10-14 first version; 2026-10-16 #2871 workflow completion and reporting; 2026-10-22 Turkish and Central European letters drawn, UI notices translated and shown only when something was approximated; 2026-10-25 review fixes (Word 2010 text boxes, nested boxes, master placeholders, merged table cells, stored column widths) and the known gaps listed; 2026-10-27 wording: what can be left out, which workflows convert off the UI thread; 2026-10-30 fidelity (#2326 follow-up): tabs and leaders, paragraph and table-style borders, header tables, floating pictures and tables, text columns, footnotes on their reference's page, shapes, text boxes and SmartArt drawn in place, equations laid out, live page, date and style-reference fields, picture crops, widow and orphan control, page borders and colour, line numbers, drop caps, links and bookmarks; Excel print setup, headers and footers, conditional formatting with formula rules, formatted tables, pictures, shapes, sparklines and charts on sheets and chart sheets, cell text rotation and rich text, number format colours, fractions and conditions, unsaved formulas calculated; EMF and WMF pictures as vectors, charts from their cached values.

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
  spacing (lines laid out as Word sets them, the line gap above the text, space before kept only at the top of the
  document's first page), widow and orphan control (on unless a paragraph turns it off, as in
  Word), keep lines together, drop caps (dropped or in the margin, the paragraph after wrapping
  beside them), left,
  right, centre and decimal tab stops with dot, hyphen, underscore and middle-dot leaders,
  paragraph borders (boxes shared by consecutive paragraphs; single, double, dotted, dashed and
  wavy rules) and shading, in table cells too, numbered
  and bulleted lists (`numbering.xml`, nine levels, restarts), tables (column grid, spans,
  vertical merges, cell shading, table-style and cell borders, the table style's header, total
  and banded rows and first and last columns (`tblLook`), cell vertical alignment, rows split
  across pages, header rows repeated on each page, tables floating against the page or its
  margins, tables floating in the text with the text after them wrapping beside them, text
  turned a quarter in cells (`btLr`, `tbRl`)), inline and anchored pictures and charts (PNG, JPEG, and EMF and WMF drawn as vectors,
  cropped as the picture says;
  anchored ones in front of or behind the text take no room, top-and-bottom ones their height,
  and text wraps beside square ones), page breaks, column breaks and text columns (equal or
  unequal, with separator lines, balanced before a continuous section break), page borders
  (from the page edge or the text, every page, the first or the rest), the page colour
  (`w:background`), line numbers (every n lines, restarting each page, section or never,
  suppressed paragraphs left out), headers and footers per section
  (first-page variants, tables laid out as tables) with live `PAGE` (in the field's `\*` or the
  section's number format, restarting where a section says), `NUMPAGES`, `SECTIONPAGES` and
  `STYLEREF` (the page's first paragraph in the style, else the last before it) fields, in the
  body's text too (its page counts from a first layout of the pages); `DATE`,
  `TIME` and `PRINTDATE` show the local date and time of the conversion in their `\@` format, as
  Word updates them when it prints,
  table-of-contents links printed without the Hyperlink style (as Word prints them), footnotes at
  the foot of the page of their reference's line and endnotes at the end (with their marks), list
  counters shared by the instances of one abstract list, charts (see Excel), equations laid out
  (fractions, scripts, radicals, sums and integrals, delimiters, matrices, equation arrays,
  accents, limits; display equations centred), shapes, groups, drawing canvases and text boxes
  drawn where they are with their fill, outline and text (rotated too, vertical text turned), text
  boxes saved only as VML drawn in place too (their fill, outline, position and insets), SmartArt
  drawn from the drawing Word saves with it, and tracked changes printed as the final text (insertions kept, deletions and moves away left
  out). Links: hyperlinks to addresses (web, mail and FTP only) and to bookmarks, table-of-contents
  entries and `REF`/`PAGEREF` cross-references with `\h` are clickable, and headings (paragraphs
  with an outline level) become the PDF's bookmarks.
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
  Portuguese month and day names, sections for negatives and zero, conditional sections such as
  `[>=100]`, fractions such as `# ?/?` and `?/8`, colour tags such as `[Red]`, `[Blue]` and
  `[Color10]`, text sections with `@`; General numbers shortened, then written in scientific
  notation, to fit a narrow column, as Excel shows them); ISO date cells (`t="d"`); rich text (runs
  with their own font, size, colour, bold, italic and super- or subscript); fonts, fills
  (patterns as the mix of their two colours), borders (dotted, dashed, double and diagonal ones
  too; a merged range's bottom and right edges from its last row and column), horizontal
  alignment including fill, justify and distributed, vertical alignment, indents, shrink to fit,
  rotated and stacked text (rows fitted to it), wrapped text (across a merged range's whole
  width), merged cells, overflow into empty neighbours and `###` for numbers that don't fit;
  formatted tables (Insert ▸ Table) in their style: the workbook's own table styles, and Excel's
  built-in Light, Medium and Dark families drawn from the theme colours (header, total row, row
  stripes, first and last column, borders); hyperlinks to addresses and to sheets.
  Formulas show their saved results; formulas saved without one (by tools that don't calculate)
  are calculated with the formula evaluator below. Print setup: headers and footers (`&L &C &R`, `&P &N &D &T
  &F &A`, fonts, sizes, colours, bold/italic/underline, first-page and odd/even variants, `&G`
  pictures from `legacyDrawingHF`), print areas (each on its own pages), print title rows and
  columns, horizontal and vertical centring, manual row and column breaks, over-then-down page
  order, a sheet's own first page number (`useFirstPageNumber`); comments printed on pages after the sheet when its page setup asks for them at the end
  (`cellComments="atEnd"`), as Excel for Windows prints them. Pictures (PNG, JPEG, EMF, WMF),
  shapes (rectangles, rounded rectangles, ellipses, arrows, triangles, diamonds, lines with
  arrowheads, with fill, outline and text), groups of them, SmartArt and sparklines (line, column
  and win/loss) on sheets, at their anchors. Charts (Word, Excel and PowerPoint, from the values cached in the chart part): bar and
  column (clustered, stacked, 100 % stacked), line, area, pie, doughnut, scatter and radar (lines
  or filled), combinations of them with a secondary value axis on the right, with title, axes,
  gridlines, legend, pie percentages, series values written at their points and linear or
  moving-average trendlines, and surface charts as
  Excel's contour view (value bands in colour); chart sheets print their chart on a landscape
  page. Conditional formatting: cell value, top/bottom N and N %, above/below average,
  duplicate and unique values, text, blanks and errors, time periods, colour scales, data bars and
  icon sets, and formula rules (`expression`: comparisons, arithmetic, `&`, relative and absolute
  references, `AND OR NOT IF IFERROR`, `MOD ABS INT ROUND`, `IS…` tests, text functions such as
  `SEARCH LEFT LEN UPPER`, `ROW COLUMN TODAY`, `SUM AVERAGE MIN MAX COUNT COUNTA COUNTIF`), by
  priority with stop-if-true. Printed row and column headings. Rows without a
  custom height are fitted again when printing (their fonts' line height, each wrapped line) and
  print on Excel's 600 dpi grid; column widths count in the digit width of the Excel that saved
  the workbook (8 px for Excel for Mac, 7 px otherwise), so a fit-to-page sheet scales as it did
  there. `&D` and `&T` print local time in the system's short date and time format (US English
  without a locale; UTC and US English in the browser).

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
- **Not drawn, shown as outlined boxes:** SmartArt saved without its drawing, embedded objects
  without an EMF, WMF or bitmap preview, and pictures in a format PdfCraft can't decode (SVG) or
  linked rather than embedded. EMF pictures are played as vectors (pens, brushes, fonts,
  rectangles, rounded rectangles, ellipses, polygons, polylines, Béziers, paths, text, mapping
  modes and transforms), and WMF pictures likewise (pens, brushes, fonts, rectangles, rounded
  rectangles, ellipses, polygons, polylines, text, window origin and extent); their bitmaps,
  clipping regions and arcs are skipped.
- **Simplified:** gradients use their middle colour; arcs in custom shapes are drawn straight;
  preset shapes without a specific outline use their bounding rectangle; vertical text in
  PowerPoint text boxes is drawn horizontally; equations are set in the serif standard face
  (Times) rather than Cambria Math; charts are drawn
  flat (3-D charts too) with Office's default look where the chart doesn't style itself; a rich
  text cell's lines are broken and spaced by its largest font; tables don't wrap around
  floating pictures (they start below one); turned table cells don't split across pages; Excel's
  built-in table styles are modelled by family rather than copied one by one, so their shades
  and lines are close to Excel's but not exact; pages are numbered through the whole workbook
  unless a sheet sets its own first page number.
- **Known gaps, not yet handled** (the source file is never changed, but some content is
  simplified or left out of the PDF, and the conversion report notes the content-specific cases):
  - Word: other fields (`REF`, `SEQ`, `NUMWORDS`, `CREATEDATE`…) keep their saved results;
    frames other than drop caps stay in the flow; comments are not printed (as with Word's "No
    markup").
  - PowerPoint: transparency (`alpha`) is ignored, so a translucent overlay is drawn opaque;
    curved and bent connectors are drawn as boxes; Wingdings bullets show as the letters they are
    stored as; text in a flipped shape is flipped with it; placeholders inherit no fill or
    outline from the layout or master.
  - Excel: a number that just fits its column in Calibri can be shown
    as `####`, as Helvetica is wider; formula rules and unsaved formulas using functions or
    references outside the list above (other sheets, names, arrays) are left out and reported;
    a 45° turned cell's borders aren't slanted as Excel slants them; comments shown on the sheet (`cellComments="asDisplayed"`) are left out
    and reported. Excel for Mac leaves comments at the end out of the PDFs it saves, so a Mac
    workbook asking for them gets a page more here than in Excel for Mac's own PDF.
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
EMF and WMF pictures, Word shapes, groups and turned text boxes, SmartArt in Word and Excel,
equations, picture crops, dotted, dashed and wavy borders, cell paragraph shading, footnotes by
reference line, live date, time, page-number and style-reference fields, Excel headers and
footers, print titles and areas, conditional formatting with formula rules, rotated, stacked and
rich cell text, format colours, ISO dates, comments at the end, radar and surface charts, sheet
pictures and shapes, links and bookmarks, widows, orphans and kept lines, page colour, borders,
line numbers and drop caps, tables floating in the text, tracked changes, VML boxes, turned text,
Excel formatted tables, patterns, diagonal and double borders,
shrink, fill and distributed text, merged edges, unsaved formulas, own first page numbers, sheet
groups, sparklines and charts with two axes, trendlines and labels, and hostile input (`emf.rs`,
`wmf.rs`, `math.rs`, `xlsx_formula.rs`,
`xlsx_hf.rs`, `xlsx_cf.rs` and `flow.rs` have their own unit tests, hostile metafiles, equations
and formulas included).
`crates/automation/tests/it/automation.rs::creating_pdfs_from_word_and_powerpoint_natively` exports a
PDF to `.docx` and `.pptx` with PdfCraft's own exporter and imports them back through `doc_create`.
