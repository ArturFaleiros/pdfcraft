# Office to PDF

The desktop app can open Word (`.docx`, `.doc`), PowerPoint (`.pptx`, `.ppt`), Excel
(`.xlsx`, `.xls`), OpenDocument (`.odt`, `.odp`, `.ods`) and `.rtf` files as PDFs.
Install LibreOffice separately, then use **File > Open** and select the
**Word / PowerPoint / Excel / LibreOffice** filter. Conversion runs in the background.
The result is a new unsaved PDF tab. **Save As** chooses its destination; the source
Office document is never overwritten. Existing PDF/image combining and PDF exports
to Word and PowerPoint remain available.

LibreOffice is optional and is not downloaded, bundled or started with PdfCraft.
Only opening an Office file or calling the conversion tool starts it. Standard
Windows Program Files, macOS Applications and Linux `/usr/bin` installations are
detected. For another installation, set `PDFCRAFT_LIBREOFFICE` to the absolute path
of its `soffice.com` (Windows) or `soffice`/`libreoffice` executable.

## Automation

`doc_convert_office` takes `path` and returns a new document id, like `doc_create`.
The engine validates the converted PDF before opening it. For example, in a CLI script:

```json
[
  {"tool":"doc_convert_office","args":{"path":"report.docx"}},
  {"tool":"doc_save","args":{"doc":1,"path":"report.pdf"}}
]
```

The tool refuses a `--root` confined session before reading the input or starting
LibreOffice: the external application cannot inherit PdfCraft's path resolver.
It is not an OS sandbox. The browser version cannot run this local converter.

Each conversion gets a temporary directory and a fresh LibreOffice profile with
macro execution disabled and Writer/Calc automatic link updates disabled. Original
filenames are not used as command-line switches. The input is copied under a fixed
name and the temporary files are removed on return. Limits are 128 MiB input,
120 seconds and 256 MiB output. Missing LibreOffice, failed conversions, empty
output and timeouts return errors and do not replace an open document.

Layout depends on LibreOffice's import filters and the fonts installed on the
machine. Check the preview before sharing: missing fonts, linked content, advanced
Office features and spreadsheet print areas can affect pagination or appearance.
Password-protected documents and macro-enabled OOXML extensions are not supported.
Office files are opened individually; combine the resulting PDFs afterwards.

The external interface follows LibreOffice's [documented command-line parameters](https://help.libreoffice.org/latest/en-GB/text/shared/guide/start_parameters.html).
No LibreOffice implementation or binary is included in this repository.

## Verification

The ordinary platform tests cover format validation, missing executables and false
success with no output. Automation tests verify that confined sessions refuse the
operation before probing input paths. Real conversion tests require an installed
LibreOffice and `PDFCRAFT_OFFICE_TEST_INPUT` pointing to a synthetic document whose
text contains `PUBLICO`:

```sh
cargo test -p pdfcraft-automation office_conversion_opens_a_new_unsaved_pdf -- --ignored
cargo test -p pdfcraft-ui-egui --lib office_open_converts_in_background_without_replacing_the_source -- --ignored
```

The UI test drives `ui.open`, waits for the background conversion, verifies that the
source is unchanged and the new tab is unsaved, and optionally writes a screenshot
to `PDFCRAFT_OFFICE_TEST_SHOT`. No fixture files are committed.
