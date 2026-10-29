//! Office import tests. Every package is generated here from synthetic text and pixels; no
//! Office file, font or image is committed.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, write_full};

use super::zip::tests::build;
use super::*;
use crate::{SourceKind, source_kind};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const S: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const T: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// These tests read the content streams the standard faces write (literal strings, `/F` fonts),
/// so they convert with embedding off: they hold with or without the craft-fonts build input.
/// The embedded faces have tests of their own.
fn from_office(name: &str, bytes: &[u8]) -> Result<(Document, OfficeReport), crate::CreateError> {
    from_office_with(name, bytes, OfficeOptions { embed_fonts: false })
}

fn rels(items: &[(&str, &str, &str)]) -> String {
    let mut s = format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{REL}">"#);
    for (id, ty, target) in items {
        // Web and mail addresses are external targets.
        let mode = if target.starts_with("http") || target.starts_with("mailto:") { r#" TargetMode="External""# } else { "" };
        s.push_str(&format!(r#"<Relationship Id="{id}" Type="{T}/{ty}" Target="{target}"{mode}/>"#));
    }
    s.push_str("</Relationships>");
    s
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(&vec![90u8; (w * h * 3) as usize]).unwrap();
    }
    out
}

fn reopen(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("parses");
    Document::open(Arc::new(bytes)).unwrap()
}

fn pages(doc: &Document) -> Vec<Dict> {
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let kids = doc.get(pages).as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap().clone();
    kids.iter().map(|k| doc.resolve(k).as_dict().cloned().unwrap()).collect()
}

fn content(doc: &Document, page: &Dict) -> String {
    let Object::Stream(s) = &*doc.resolve(page.get(b"Contents").unwrap()) else { panic!("no content") };
    // WinAnsi bytes as characters (Latin-1 agrees with WinAnsi from 0xA0 up).
    s.decoded().unwrap().iter().map(|b| char::from(*b)).collect()
}

fn all_text(doc: &Document) -> String {
    pages(doc).iter().map(|p| content(doc, p)).collect::<Vec<_>>().join("\n")
}

fn size(p: &Dict) -> (f64, f64) {
    let m: Vec<f64> = p.get(b"MediaBox").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    (m[2], m[3])
}

// ── Word ────────────────────────────────────────────────────────────────────────────────────

fn docx(body: &str, extra: &[(&str, &[u8])], doc_rels: &[(&str, &str, &str)]) -> Vec<u8> {
    docx_numbered(body, extra, doc_rels, None)
}

/// [`docx`] with `numbering` (a whole `numbering.xml`) in place of the standard one when given.
fn docx_numbered(body: &str, extra: &[(&str, &[u8])], doc_rels: &[(&str, &str, &str)], numbering: Option<&str>) -> Vec<u8> {
    // A page colour (`w:background`) at the start of `body` goes before the body, where it belongs.
    let (background, body) = match body.strip_prefix("<w:background").and_then(|rest| rest.split_once("/>")) {
        Some((attrs, rest)) => (format!("<w:background{attrs}/>"), rest),
        None => (String::new(), body),
    };
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="{R}" xmlns:a="{A}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">{background}<w:body>{body}</w:body></w:document>"#
    );
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
        <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
        <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:before="240"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:rFonts w:ascii="Cambria"/><w:b/><w:sz w:val="32"/><w:color w:val="2F5496"/></w:rPr></w:style>
        <w:style w:type="table" w:styleId="TableGrid"><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4"/><w:left w:val="single" w:sz="4"/><w:bottom w:val="single" w:sz="4"/><w:right w:val="single" w:sz="4"/><w:insideH w:val="single" w:sz="4"/><w:insideV w:val="single" w:sz="4"/></w:tblBorders></w:tblPr></w:style>
        <w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style>
        <w:style w:type="table" w:styleId="Banded"><w:name w:val="Banded"/><w:tblStylePr w:type="firstRow"><w:rPr><w:b/><w:color w:val="FFFFFF"/></w:rPr><w:tcPr><w:shd w:val="clear" w:fill="4472C4"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="band1Horz"><w:tcPr><w:shd w:val="clear" w:fill="D9E2F3"/></w:tcPr></w:tblStylePr></w:style>
        <w:style w:type="paragraph" w:styleId="Loop"><w:basedOn w:val="Loop2"/></w:style><w:style w:type="paragraph" w:styleId="Loop2"><w:basedOn w:val="Loop"/></w:style></w:styles>"#
    );
    let standard = format!(
        r#"<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2)"/></w:lvl></w:abstractNum>
        <w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="&#xF0B7;"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum>
        <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num></w:numbering>"#
    );
    let numbering = numbering.map_or(standard, str::to_owned);
    let mut r = vec![("rIdS", "styles", "styles.xml"), ("rIdN", "numbering", "numbering.xml")];
    r.extend_from_slice(doc_rels);
    let doc_rels = rels(&r);
    let root_rels = rels(&[("rId1", "officeDocument", "word/document.xml")]);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", b"<Types/>"),
        ("_rels/.rels", root_rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/_rels/document.xml.rels", doc_rels.as_bytes()),
        ("word/styles.xml", styles.as_bytes()),
        ("word/numbering.xml", numbering.as_bytes()),
    ];
    parts.extend_from_slice(extra);
    build(&parts)
}

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

#[test]
fn word_paragraphs_styles_lists_tables_images_and_page_numbers() {
    let image = png(40, 20);
    let header = format!(
        r#"<w:hdr xmlns:w="{W}"><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>9</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> of </w:t></w:r><w:fldSimple w:instr="NUMPAGES"><w:r><w:t>9</w:t></w:r></w:fldSimple></w:p></w:hdr>"#
    );
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Relatório Anual</w:t></w:r></w:p>
        <w:p><w:r><w:t xml:space="preserve">Texto com </w:t></w:r><w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>negrito</w:t></w:r><w:r><w:t xml:space="preserve"> e ação, coração, São Paulo.</w:t></w:r></w:p>
        <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Primeiro</w:t></w:r></w:p>
        <w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Sub</w:t></w:r></w:p>
        <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Segundo</w:t></w:r></w:p>
        <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>Marcador</w:t></w:r></w:p>
        <w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>
          <w:tr><w:tc><w:tcPr><w:shd w:fill="D9E2F3"/></w:tcPr>{}</w:tc><w:tc>{}</w:tc></w:tr>
          <w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr>{}</w:tc></w:tr></w:tbl>
        <w:p><w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="635000"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rIdImg"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
        <w:p><w:r><w:t>中文</w:t></w:r><w:r><w:br w:type="page"/></w:r><w:r><w:t>Depois da quebra</w:t></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="Loop"/></w:pPr><w:r><w:t>Estilo em ciclo</w:t></w:r></w:p>
        <w:sectPr><w:headerReference w:type="default" r:id="rIdH"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708"/></w:sectPr>"#,
        p("Nome"),
        p("Valor"),
        p("Linha inteira")
    );
    let bytes = docx(
        &body,
        &[("word/media/image1.png", &image), ("word/header1.xml", header.as_bytes())],
        &[("rIdImg", "image", "media/image1.png"), ("rIdH", "header", "header1.xml")],
    );
    let (doc, report) = from_office("Relatorio.docx", &bytes).unwrap();
    assert_eq!(report.kind, OfficeKind::Word);
    assert_eq!(report.pages, 2, "{report:?}");
    assert_eq!(report.escaped, 2, "the two Chinese characters are written as code points: {report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert_eq!(pg.len(), 2);
    let (w, h) = size(&pg[0]);
    assert!((w - 595.3).abs() < 0.2 && (h - 841.9).abs() < 0.2, "A4 from the section: {w}x{h}");
    let first = content(&doc, &pg[0]);
    let second = content(&doc, &pg[1]);
    for needle in [
        "(Relatório)",
        "(negrito)",
        "(ação,)",
        "(coração,)",
        "(1.)",
        "(a\\))",
        "(2.)",
        "(Primeiro)",
        "(Nome)",
        "(Valor)",
        "(inteira)",
        "([U+4E2D][U+6587])",
        "/Im0 Do",
    ] {
        assert!(first.contains(needle), "missing {needle} on page 1");
    }
    assert!(first.contains("(\u{95})"), "the Symbol bullet is drawn as a bullet");
    assert!(second.contains("(quebra)"), "text after the page break starts page 2");
    assert!(first.contains("(1)") && first.contains("(2)"), "header page number and count on page 1: {first}");
    assert!(second.contains("(2)"), "header page number on page 2");
    // Heading in Times (Cambria is a serif face), body in Helvetica (Calibri).
    let fonts = pg[0].get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
    let names: Vec<String> =
        fonts.iter().map(|(_, r)| String::from_utf8_lossy(doc.resolve(r).as_dict().unwrap().name(b"BaseFont").unwrap()).into_owned()).collect();
    assert!(
        names.iter().any(|n| n == "Times-Bold") && names.iter().any(|n| n == "Helvetica") && names.iter().any(|n| n == "Helvetica-BoldOblique"),
        "{names:?}"
    );
}

#[test]
fn headings_keep_with_their_next_paragraph_and_lists_drop_contextual_spacing() {
    // Enough body text to leave a little room at the bottom of page 1, then a heading.
    let filler: String = (0..41).map(|i| p(&format!("Linha de enchimento {i}"))).collect();
    let keep = r#"<w:p><w:pPr><w:keepNext/><w:spacing w:before="0" w:after="0"/></w:pPr><w:r><w:t>Titulo preso</w:t></w:r></w:p>"#;
    let tall = r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0"/></w:pPr><w:r><w:t>Corpo</w:t></w:r><w:r><w:br/><w:t>a</w:t><w:br/><w:t>b</w:t><w:br/><w:t>c</w:t><w:br/><w:t>d</w:t></w:r></w:p>"#;
    let (doc, _) = from_office("k.docx", &docx(&format!("{filler}{keep}{tall}"), &[], &[])).unwrap();
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert_eq!(pg.len(), 2);
    assert!(content(&doc, &pg[1]).contains("(preso)"), "the heading moves to the next page with its paragraph");
    assert!(!content(&doc, &pg[0]).contains("(preso)"));

    // Two list items with contextual spacing sit closer than two plain paragraphs.
    let item = |t: &str, ctx: bool| {
        let c = if ctx { "<w:contextualSpacing/>" } else { "" };
        format!(r#"<w:p><w:pPr><w:spacing w:after="400"/>{c}</w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#)
    };
    let gap = |ctx: bool| {
        let (doc, _) = from_office("c.docx", &docx(&format!("{}{}", item("Um", ctx), item("Dois", ctx)), &[], &[])).unwrap();
        let text = content(&reopen(&doc), &pages(&reopen(&doc))[0]);
        let y = |w: &str| -> f64 {
            let at = text.find(&format!("({w})")).unwrap();
            let tm = text[..at].rfind(" Tm").unwrap();
            text[..tm].split_whitespace().last().unwrap().parse().unwrap()
        };
        y("Um") - y("Dois")
    };
    assert!(gap(false) - gap(true) > 19.0, "contextual spacing removes the 20 pt after: {} vs {}", gap(false), gap(true));
}

#[test]
fn long_documents_paginate_and_tables_split_across_pages() {
    let many: String = (0..120).map(|i| p(&format!("Paragrafo numero {i} com algum texto para ocupar a linha inteira da pagina"))).collect();
    let rows: String = (0..80).map(|i| format!("<w:tr><w:tc>{}</w:tc></w:tr>", p(&format!("Celula {i}")))).collect();
    let tall = format!(
        "<w:tbl><w:tblGrid><w:gridCol w:w=\"5000\"/></w:tblGrid><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>",
        (0..90).map(|i| p(&format!("Linha alta {i}"))).collect::<String>()
    );
    let body = format!("{many}<w:tbl><w:tblGrid><w:gridCol w:w=\"5000\"/></w:tblGrid>{rows}</w:tbl>{tall}");
    let (doc, report) = from_office("long.docx", &docx(&body, &[], &[])).unwrap();
    assert!(report.pages >= 6, "{report:?}");
    let text = all_text(&reopen(&doc));
    for needle in ["(119)", "(Celula) Tj", "(79)", "(89)"] {
        assert!(text.contains(needle), "missing {needle}");
    }
}

// ── PowerPoint ──────────────────────────────────────────────────────────────────────────────

fn pptx(slides: &[&str], hidden: &[bool]) -> Vec<u8> {
    pptx_with_master(slides, hidden, "")
}

/// [`pptx`] with `master_extra` (shapes) added to the slide master's shape tree.
fn pptx_with_master(slides: &[&str], hidden: &[bool], master_extra: &str) -> Vec<u8> {
    pptx_full(slides, hidden, master_extra, "")
}

/// [`pptx_with_master`] with `pres_attrs` (attributes, e.g. ` firstSlideNum="0"`) on `p:presentation`.
fn pptx_full(slides: &[&str], hidden: &[bool], master_extra: &str, pres_attrs: &str) -> Vec<u8> {
    let theme = format!(
        r#"<a:theme xmlns:a="{A}"><a:themeElements><a:clrScheme name="t"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1></a:clrScheme><a:fontScheme name="f"><a:majorFont><a:latin typeface="Georgia"/></a:majorFont><a:minorFont><a:latin typeface="Arial"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#
    );
    let master = format!(
        r#"<p:sldMaster xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}"><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="FAFAFA"/></a:solidFill></p:bgPr></p:bg><p:spTree>
        <p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="274638"/><a:ext cx="8229600" cy="1143000"/></a:xfrm></p:spPr></p:sp>
        <p:sp><p:nvSpPr><p:cNvPr id="3" name="Body"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="1600200"/><a:ext cx="8229600" cy="4525963"/></a:xfrm></p:spPr></p:sp>
        <p:sp><p:nvSpPr><p:cNvPr id="4" name="Band"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="6629400"/><a:ext cx="9144000" cy="228600"/></a:xfrm><a:prstGeom prst="rect"/><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></p:spPr></p:sp>
        {master_extra}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1"/>
        <p:txStyles><p:titleStyle><a:lvl1pPr algn="ctr"><a:defRPr sz="4400"/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL="342900" indent="-342900"><a:buChar char="•"/><a:defRPr sz="2800"/></a:lvl1pPr><a:lvl2pPr marL="742950" indent="-285750"><a:buChar char="–"/><a:defRPr sz="2400"/></a:lvl2pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#
    );
    let layout = format!(
        r#"<p:sldLayout xmlns:p="{P}" xmlns:a="{A}"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="T"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp></p:spTree></p:cSld></p:sldLayout>"#
    );
    let mut ids = String::new();
    let mut pres_rels = Vec::new();
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    for (i, body) in slides.iter().enumerate() {
        let show = if hidden.get(i).copied().unwrap_or(false) { r#" show="0""# } else { "" };
        ids.push_str(&format!(r#"<p:sldId id="{}" r:id="rIdS{i}"/>"#, 256 + i));
        pres_rels.push((format!("rIdS{i}"), format!("slides/slide{}.xml", i + 1)));
        parts.push((
            format!("ppt/slides/slide{}.xml", i + 1),
            format!(r#"<p:sld xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}"{show}><p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sld>"#).into_bytes(),
        ));
        parts.push((
            format!("ppt/slides/_rels/slide{}.xml.rels", i + 1),
            rels(&[("rIdL", "slideLayout", "../slideLayouts/slideLayout1.xml"), ("rIdI", "image", "../media/image1.png")]).into_bytes(),
        ));
    }
    let pres = format!(
        r#"<p:presentation xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}"{pres_attrs}><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/></p:presentation>"#
    );
    let pr: Vec<(&str, &str, &str)> = pres_rels.iter().map(|(id, t)| (id.as_str(), "slide", t.as_str())).collect();
    parts.push(("ppt/presentation.xml".into(), pres.into_bytes()));
    parts.push(("ppt/_rels/presentation.xml.rels".into(), rels(&pr).into_bytes()));
    parts.push(("_rels/.rels".into(), rels(&[("rId1", "officeDocument", "ppt/presentation.xml")]).into_bytes()));
    parts.push(("ppt/slideLayouts/slideLayout1.xml".into(), layout.into_bytes()));
    parts.push((
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels".into(),
        rels(&[("rIdM", "slideMaster", "../slideMasters/slideMaster1.xml")]).into_bytes(),
    ));
    parts.push(("ppt/slideMasters/slideMaster1.xml".into(), master.into_bytes()));
    parts.push(("ppt/slideMasters/_rels/slideMaster1.xml.rels".into(), rels(&[("rIdT", "theme", "../theme/theme1.xml")]).into_bytes()));
    parts.push(("ppt/theme/theme1.xml".into(), theme.into_bytes()));
    parts.push(("ppt/media/image1.png".into(), png(30, 30)));
    let refs: Vec<(&str, &[u8])> = parts.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    build(&refs)
}

#[test]
fn powerpoint_slides_with_inherited_placeholders_bullets_shapes_and_pictures() {
    let title = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang="pt-BR"/><a:t>Resultados</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let bullets = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Content"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr><a:normAutofit fontScale="90000"/></a:bodyPr><a:p><a:r><a:t>Vendas subiram</a:t></a:r></a:p><a:p><a:pPr lvl="1"/><a:r><a:t>no Brasil</a:t></a:r></a:p><a:p><a:r><a:t>Slide </a:t></a:r><a:fld id="{1}" type="slidenum"><a:t>9</a:t></a:fld></a:p></p:txBody></p:sp>"#;
    let shape = r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="5400000"><a:off x="6000000" y="5000000"/><a:ext cx="2000000" cy="500000"/></a:xfrm><a:prstGeom prst="roundRect"/><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></p:spPr><p:txBody><a:bodyPr anchor="ctr"/><a:p><a:pPr algn="ctr"/><a:r><a:rPr b="1"/><a:t>Destaque</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let pic = r#"<p:pic><p:nvPicPr><p:cNvPr id="6" name="Pic"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdI"/><a:srcRect l="10000" r="10000"/></p:blipFill><p:spPr><a:xfrm><a:off x="100000" y="100000"/><a:ext cx="500000" cy="500000"/></a:xfrm></p:spPr></p:pic>"#;
    let group = r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="7" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="1000000" y="1000000"/><a:ext cx="2000000" cy="2000000"/><a:chOff x="0" y="0"/><a:chExt cx="1000000" cy="1000000"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="8" name="In"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1000000" cy="300000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr wrap="none"/><a:p><a:r><a:t>Agrupado</a:t></a:r></a:p></p:txBody></p:sp></p:grpSp>"#;
    let s1 = format!("{title}{bullets}{shape}{pic}{group}");
    let s2 = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Hidden"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Oculto</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let s3 = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="9" name="Chart"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="2000000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"/></a:graphic></p:graphicFrame>
      <p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="3500000" y="0"/><a:ext cx="4000000" cy="800000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1" bandRow="1"><a:tableStyleId>{5C22544A}</a:tableStyleId></a:tblPr><a:tblGrid><a:gridCol w="2000000"/><a:gridCol w="2000000"/></a:tblGrid><a:tr h="370840"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Mes</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Total</a:t></a:r></a:p></a:txBody></a:tc></a:tr><a:tr h="370840"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Jan</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>42</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#;
    let bytes = pptx(&[&s1, s2, s3], &[false, true, false]);
    let (doc, report) = from_office("deck.pptx", &bytes).unwrap();
    assert_eq!(report.kind, OfficeKind::PowerPoint);
    assert_eq!(report.pages, 2, "the hidden slide is left out: {report:?}");
    assert!(report.notes.iter().any(|n| n.contains("hidden slides")) && report.notes.iter().any(|n| n.contains("charts")), "{report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert_eq!(size(&pg[0]), (720.0, 540.0));
    let s1 = content(&doc, &pg[0]);
    for needle in
        ["(Resultados)", "(Vendas)", "(subiram)", "(\u{95})", "(\u{96})", "(Brasil)", "(1)", "(Destaque)", "/Im0 Do", "(Agrupado)", " re W n"]
    {
        assert!(s1.contains(needle), "missing {needle} on slide 1");
    }
    assert!(!all_text(&doc).contains("Oculto"));
    // The title inherits the master's position and 44 pt centred style in the theme's major font.
    assert!(s1.contains(" 44 Tf"), "title size from the master text style");
    let fonts = pg[0].get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
    let names: Vec<String> =
        fonts.iter().map(|(_, r)| String::from_utf8_lossy(doc.resolve(r).as_dict().unwrap().name(b"BaseFont").unwrap()).into_owned()).collect();
    assert!(names.contains(&"Times-Roman".to_string()) && names.contains(&"Helvetica".to_string()), "{names:?}");
    // The body was scaled by its 90 % autofit: 28 pt becomes 25.2 pt.
    assert!(s1.contains(" 25.2 Tf"), "autofit scale");
    // The master's accent band is drawn under the slide (scheme colour accent1 = 4472C4).
    assert!(s1.contains("0.267 0.447 0.769 rg"), "master decoration in the theme colour");
    let s3 = content(&doc, &pg[1]);
    assert!(s3.contains("(Mes)") && s3.contains("(42)"), "table cells");
    assert!(s3.contains("1 1 1 rg") && s3.contains("0.267 0.447 0.769 rg"), "header row: white text on the accent colour: {s3}");
}

// ── Excel ───────────────────────────────────────────────────────────────────────────────────

fn xlsx(sheet: &str, extra_sheets: &[(&str, &str, bool)]) -> Vec<u8> {
    xlsx_with(sheet, extra_sheets, "")
}

/// [`xlsx`] with `workbook_pr` (a `<workbookPr>` element, or nothing) in the workbook.
fn xlsx_with(sheet: &str, extra_sheets: &[(&str, &str, bool)], workbook_pr: &str) -> Vec<u8> {
    xlsx_styled(sheet, extra_sheets, workbook_pr, r"[$R$-416]\ #,##0.00")
}

/// [`xlsx_with`] with `format` as the number format of style 1 (`s="1"`).
fn xlsx_styled(sheet: &str, extra_sheets: &[(&str, &str, bool)], workbook_pr: &str, format: &str) -> Vec<u8> {
    xlsx_full(sheet, extra_sheets, workbook_pr, format, None, &[])
}

/// [`xlsx_styled`] with `shared` (the `<si>` elements) as the shared strings when given, and
/// `extra` parts (drawings, pictures, relationships) added to the package.
fn xlsx_full(
    sheet: &str,
    extra_sheets: &[(&str, &str, bool)],
    workbook_pr: &str,
    format: &str,
    shared: Option<&str>,
    extra: &[(&str, &[u8])],
) -> Vec<u8> {
    let styles = format!(
        r#"<styleSheet xmlns="{S}"><numFmts count="2"><numFmt numFmtId="164" formatCode="{format}"/><numFmt numFmtId="165" formatCode="dd/mm/yyyy"/></numFmts>
        <fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><color rgb="FFFFFFFF"/><name val="Calibri"/></font></fonts>
        <fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor theme="4"/></patternFill></fill><fill><patternFill patternType="darkGray"><fgColor rgb="FF000000"/><bgColor rgb="FFFFFFFF"/></patternFill></fill></fills>
        <borders count="2"><border/><border><left style="thin"/><right style="thin"/><top style="thin"/><bottom style="medium"><color rgb="FF0000FF"/></bottom></border><border><bottom style="dotted"/></border><border diagonalUp="1"><left style="double"/><diagonal style="thin"><color rgb="FF00FF00"/></diagonal></border></borders>
        <cellXfs count="7"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="164" fontId="0" fillId="0" borderId="1" applyNumberFormat="1"/><xf numFmtId="165" fontId="0" fillId="0" borderId="0"/><xf numFmtId="0" fontId="1" fillId="2" borderId="1"><alignment horizontal="center"/></xf><xf numFmtId="10" fontId="0" fillId="0" borderId="0"/><xf numFmtId="0" fontId="0" fillId="0" borderId="0"><alignment wrapText="1"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="0"><alignment horizontal="general"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="0"><alignment textRotation="90"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="0"><alignment textRotation="255"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="2"/><xf numFmtId="0" fontId="0" fillId="3" borderId="0"/><xf numFmtId="0" fontId="0" fillId="0" borderId="3"/><xf numFmtId="0" fontId="0" fillId="0" borderId="0"><alignment shrinkToFit="1"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="0"><alignment horizontal="fill"/></xf><xf numFmtId="0" fontId="0" fillId="0" borderId="0"><alignment horizontal="distributed"/></xf></cellXfs><dxfs count="1"><dxf><font><b/></font><fill><patternFill><bgColor rgb="FFFFC7CE"/></patternFill></fill></dxf></dxfs></styleSheet>"#
    );
    let shared = match shared {
        Some(items) => format!(r#"<sst xmlns="{S}">{items}</sst>"#),
        None => format!(
            r#"<sst xmlns="{S}" count="3"><si><t>Produto</t></si><si><t>Preço</t></si><si><r><t>Caf</t></r><r><rPr><b/></rPr><t>é</t></r><rPh><t>X</t></rPh></si></sst>"#
        ),
    };
    let mut sheets = r#"<sheet name="Vendas" sheetId="1" r:id="rId1"/>"#.to_string();
    let mut wb_rels = vec![("rId1".to_string(), "worksheet", "worksheets/sheet1.xml".to_string())];
    let mut parts: Vec<(String, Vec<u8>)> =
        vec![("xl/worksheets/sheet1.xml".into(), format!(r#"<worksheet xmlns="{S}" xmlns:r="{R}">{sheet}</worksheet>"#).into_bytes())];
    for (i, (name, body, hidden)) in extra_sheets.iter().enumerate() {
        let id = format!("rIdX{i}");
        let state = if *hidden { r#" state="hidden""# } else { "" };
        sheets.push_str(&format!(r#"<sheet name="{name}" sheetId="{}" r:id="{id}"{state}/>"#, i + 2));
        // A sheet named "Macro…" is related as an Excel 4 macro sheet, not a worksheet.
        let ty = if name.starts_with("Macro") {
            "xlMacrosheet"
        } else if name.starts_with("Grafico") {
            // A sheet named "Grafico…" is related as a chart sheet.
            "chartsheet"
        } else {
            "worksheet"
        };
        wb_rels.push((id, ty, format!("worksheets/sheet{}.xml", i + 2)));
        parts.push((format!("xl/worksheets/sheet{}.xml", i + 2), format!(r#"<worksheet xmlns="{S}" xmlns:r="{R}">{body}</worksheet>"#).into_bytes()));
    }
    let wb = format!(r#"<workbook xmlns="{S}" xmlns:r="{R}">{workbook_pr}<sheets>{sheets}</sheets></workbook>"#);
    let mut r: Vec<(&str, &str, &str)> = wb_rels.iter().map(|(a, b, c)| (a.as_str(), *b, c.as_str())).collect();
    r.push(("rIdSt", "styles", "styles.xml"));
    r.push(("rIdSs", "sharedStrings", "sharedStrings.xml"));
    parts.push(("xl/workbook.xml".into(), wb.into_bytes()));
    parts.push(("xl/_rels/workbook.xml.rels".into(), rels(&r).into_bytes()));
    parts.push(("xl/styles.xml".into(), styles.into_bytes()));
    parts.push(("xl/sharedStrings.xml".into(), shared.into_bytes()));
    parts.push(("_rels/.rels".into(), rels(&[("rId1", "officeDocument", "xl/workbook.xml")]).into_bytes()));
    parts.extend(extra.iter().map(|(n, b)| ((*n).to_owned(), b.to_vec())));
    let refs: Vec<(&str, &[u8])> = parts.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    build(&refs)
}

#[test]
fn excel_cell_text_turns_stacks_mixes_fonts_colours_dates_and_prints_comments_at_the_end() {
    let sheet = r#"<cols><col min="1" max="1" width="20" customWidth="1"/></cols><sheetData>
      <row r="1"><c r="A1" s="1"><v>5</v></c><c r="B1" s="7" t="inlineStr"><is><t>Girado</t></is></c><c r="C1" s="9" t="inlineStr"><is><t>Pontos</t></is></c></row>
      <row r="2"><c r="A2" s="1"><v>-3</v></c><c r="B2" s="8" t="inlineStr"><is><t>Pilha</t></is></c></row>
      <row r="3"><c r="A3" s="1" t="inlineStr"><is><t>Texto</t></is></c></row>
      <row r="4"><c r="A4" s="2" t="d"><v>2024-03-15T12:00:00</v></c></row>
      <row r="5"><c r="A5" t="s"><v>0</v></c></row></sheetData>
      <pageSetup cellComments="atEnd"/>"#;
    let shared = r#"<si><r><t>H</t></r><r><rPr><vertAlign val="subscript"/><sz val="11"/></rPr><t>2</t></r><r><t>O </t></r><r><rPr><b/><color rgb="FFFF0000"/></rPr><t>Forte</t></r></si>"#;
    let comments = format!(
        r#"<comments xmlns="{S}"><authors><author>Autor</author></authors><commentList><comment ref="A1" authorId="0"><text><r><rPr><b/></rPr><t>Autor:</t></r><r><t xml:space="preserve">
Comentario aqui</t></r></text></comment></commentList></comments>"#
    );
    let rels_xml = sheet_rels(&[("rIdC", "comments", "../comments1.xml")]);
    let extra: [(&str, &[u8]); 2] = [("xl/worksheets/_rels/sheet1.xml.rels", rels_xml.as_bytes()), ("xl/comments1.xml", comments.as_bytes())];
    let format = "[Blue]0.00;[Color10]-0.00;0;[Magenta]&quot;Nota &quot;@";
    let (doc, report) = from_office("t.xlsx", &xlsx_full(sheet, &[], "", format, Some(shared), &extra)).unwrap();
    assert_eq!(report.pages, 2, "the sheet and its comments page: {report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    let text = content(&doc, &pg[0]);
    assert_eq!(colour_of(&text, "5.00"), "0 0 1", "[Blue]: {text}");
    assert_eq!(colour_of(&text, "-3.00"), "0 0.502 0", "[Color10], the palette's dark green");
    assert!(text.contains("(Nota Texto)"), "the text section's literal: {text}");
    assert_eq!(colour_of(&text, "Nota Texto"), "1 0 1", "the text section's [Magenta]");
    assert!(text.contains("(15/03/2024)"), "an ISO date cell in its date format: {text}");
    assert!(text.contains("(H)") && text.contains("(2)") && text.contains("(O )"), "rich runs drawn apart: {text}");
    assert_eq!(colour_of(&text, "Forte"), "1 0 0", "the run's own colour");
    assert!(y_of(&text, "2") < y_of(&text, "H"), "the subscript sits lower");
    let turned = text.find("(Girado)").unwrap();
    assert!(text[..turned].contains("0 1 -1 0 ") && text[..turned].ends_with("1 0 0 1 0 0 Tm /F0 11 Tf "), "a cell turned a quarter: {text}");
    assert!(y_of(&text, "P") > y_of(&text, "a"), "stacked letters top to bottom");
    assert!(text.contains("[1 1] 0 d"), "a dotted border: {text}");
    let notes = content(&doc, &pg[1]);
    for needle in ["(Cell: A1)", "(Comment: Autor:)", "(Comentario aqui)"] {
        assert!(notes.contains(needle), "missing {needle}: {notes}");
    }
}

#[test]
fn excel_tables_patterns_diagonals_double_borders_shrink_fill_and_distributed_text() {
    let long = "Muito comprido para caber aqui de forma nenhuma, nem com muito boa vontade";
    let sheet = format!(
        r#"<cols><col min="1" max="1" width="24" customWidth="1"/></cols><sheetData>
      <row r="1"><c r="A1" s="10" t="inlineStr"><is><t>Padrao</t></is></c><c r="C1" t="inlineStr"><is><t>Nome</t></is></c><c r="D1" t="inlineStr"><is><t>Valor</t></is></c></row>
      <row r="2"><c r="A2" s="11" t="inlineStr"><is><t>Diagonal</t></is></c><c r="C2" t="inlineStr"><is><t>Um</t></is></c><c r="D2"><v>1</v></c></row>
      <row r="3"><c r="A3" s="12" t="inlineStr"><is><t>{long}</t></is></c><c r="C3" t="inlineStr"><is><t>Dois</t></is></c><c r="D3"><v>2</v></c></row>
      <row r="4"><c r="A4" s="13" t="inlineStr"><is><t>ab</t></is></c></row>
      <row r="5"><c r="A5" s="14" t="inlineStr"><is><t>Lorem ipsum</t></is></c></row>
      <row r="7"><c r="A7" t="inlineStr"><is><t>Junto</t></is></c></row><row r="8"><c r="A8" s="1"/></row></sheetData><mergeCells count="1"><mergeCell ref="A7:B8"/></mergeCells><tableParts count="1"><tablePart r:id="rIdT"/></tableParts>"#
    );
    let table = format!(
        r#"<table xmlns="{S}" id="1" name="Tabela1" displayName="Tabela1" ref="C1:D3" headerRowCount="1"><tableColumns count="2"><tableColumn id="1" name="Nome"/><tableColumn id="2" name="Valor"/></tableColumns><tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/></table>"#
    );
    let rels_xml = sheet_rels(&[("rIdT", "table", "../tables/table1.xml")]);
    let extra: [(&str, &[u8]); 2] = [("xl/worksheets/_rels/sheet1.xml.rels", rels_xml.as_bytes()), ("xl/tables/table1.xml", table.as_bytes())];
    let (doc, _) = from_office("t.xlsx", &xlsx_full(&sheet, &[], "", "0", None, &extra)).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    // The table: a white bold header on the accent (black without a theme), the first data row
    // striped, lines between rows in the accent's 40 % tint.
    assert_eq!(colour_of(&text, "Nome"), "1 1 1", "the header's white text: {text}");
    assert!(text.contains("/F1 11 Tf (Nome)"), "and bold");
    assert_eq!(text.matches("0.8 0.8 0.8 rg").count(), 2, "the first data row's two cells striped: {text}");
    assert!(text.contains("0.4 0.4 0.4 RG"), "the table's row lines");
    assert!(text.contains("0.25 0.25 0.25 rg"), "darkGray's 75 % black over white");
    assert!(text.contains("0 1 0 RG"), "the green diagonal");
    assert!(text.matches("0 0 0 RG 0.5 w").count() >= 2, "the double left border's two lines");
    assert!(text.contains(&format!("({long})")) && !text.contains(&format!("/F0 11 Tf ({long})")), "the long text set smaller to fit");
    assert!(text.contains("(abababab"), "fill repeats the text: {text}");
    assert!(x_of(&text, "ipsum") - x_of(&text, "Lorem") > 40.0, "distributed words spread apart");
    assert!(text.contains("0 0 1 RG 1 w"), "a merged range's bottom edge from its last row's cell");
}

#[test]
fn formulas_saved_without_results_are_calculated_and_sheets_number_their_own_pages() {
    let sheet = r#"<sheetData><row r="1"><c r="A1"><v>2</v></c><c r="B1"><v>3</v></c><c r="C1"><f>A1*B1+1</f></c><c r="D1" t="str"><f>IF(C1&gt;5,"grande","pequeno")</f></c><c r="E1"><f>NAOEXISTE(A1)</f></c></row></sheetData>
      <pageSetup useFirstPageNumber="1" firstPageNumber="7"/><headerFooter><oddFooter>&amp;CPagina &amp;P</oddFooter></headerFooter>"#;
    let (doc, report) = from_office("f.xlsx", &xlsx(sheet, &[])).unwrap();
    assert_eq!(report.notes.iter().filter(|n| n.contains("formulas without")).count(), 1, "only the unknown function is left out: {report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(7)") && text.contains("(grande)"), "the calculated results: {text}");
    assert!(text.contains("(Pagina 7)") || text.contains("Pagina 7"), "the sheet's own first page number: {text}");
}

#[test]
fn excel_sheets_with_formats_fills_borders_and_merges() {
    let sheet = r#"<cols><col min="1" max="1" width="20" customWidth="1"/><col min="2" max="4" width="14" customWidth="1"/></cols><sheetData>
      <row r="1"><c r="A1" t="s" s="3"><v>0</v></c><c r="B1" t="s" s="3"><v>1</v></c></row>
      <row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2" s="1"><v>1234.5</v></c><c r="C2" s="2"><v>45000</v></c><c r="D2" s="4"><v>0.1234</v></c></row>
      <row r="3"><c r="A3" t="inlineStr"><is><t>Total</t></is></c><c r="B3" s="1"><f>SUM(B2)</f><v>1234.5</v></c><c r="C3" t="b"><v>1</v></c><c r="D3"><f>A1*2</f></c></row>
      <row r="5"><c r="A5" t="str"><v>Mesclado</v></c></row>
      <row r="40"><c r="A40"><v>3.14159</v></c></row>
    </sheetData><mergeCells count="1"><mergeCell ref="A5:C5"/></mergeCells>"#;
    let wide = r#"<sheetData><row r="1"><c r="A1"><v>1</v></c><c r="Z1"><v>26</v></c></row></sheetData><printOptions gridLines="1"/>"#;
    let bytes = xlsx(
        sheet,
        &[
            ("Wide", wide, false),
            ("Secret", r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Escondido</t></is></c></row></sheetData>"#, true),
        ],
    );
    let (doc, report) = from_office("planilha.xlsx", &bytes).unwrap();
    assert_eq!(report.kind, OfficeKind::Excel);
    assert!(report.pages >= 3, "Vendas fits one page; Wide spreads over two: {report:?}");
    assert!(report.notes.iter().any(|n| n.contains("hidden sheets")) && report.notes.iter().any(|n| n.contains("formulas without")), "{report:?}");
    let doc = reopen(&doc);
    let text = all_text(&doc);
    for needle in
        ["(Produto)", "(Preço)", "(Caf)", "(é)", "(R$ 1.234,50)", "(15/03/2023)", "(12.34%)", "(TRUE)", "(Total)", "(Mesclado)", "(3.14159)", "(26)"]
    {
        assert!(text.contains(needle), "missing {needle}");
    }
    assert!(!text.contains("Escondido") && !text.contains("(X)"), "hidden sheets and phonetic runs are not printed");
    // Header cells: theme accent fill, white bold text, and the blue medium bottom border.
    assert!(text.contains("0 0 1 RG 1 w"), "border colour and weight");
}

#[test]
fn shape_style_font_colour_beats_inherited_text_colour() {
    let shape = r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="800000"/></a:xfrm><a:prstGeom prst="rect"/><a:solidFill><a:srgbClr val="2E7D32"/></a:solidFill></p:spPr><p:style><a:lnRef idx="1"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="3"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="2"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style><p:txBody><a:bodyPr anchor="ctr"/><a:p><a:r><a:t>Branco</a:t></a:r><a:r><a:rPr><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:rPr><a:t>Vermelho</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let (doc, _) = from_office("s.pptx", &pptx(&[shape], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("1 1 1 rg 1 0 0 1") && text.contains("(Branco)"), "the style's lt1 font colour");
    assert!(text.contains("1 0 0 rg 1 0 0 1") && text.contains("(Vermelho)"), "a run's own colour still wins");
}

#[test]
fn excel_fit_to_page_shrinks_wide_sheets_onto_one_page() {
    let cols = r#"<cols><col min="1" max="8" width="30" customWidth="1"/></cols>"#;
    let row: String = (0..8).map(|i| format!(r#"<c r="{}1" t="inlineStr"><is><t>Coluna{i}</t></is></c>"#, (b'A' + i) as char)).collect();
    let body = |fit: &str| format!(r#"{fit}{cols}<sheetData><row r="1">{row}</row></sheetData>"#);
    let (_, plain) = from_office("w.xlsx", &xlsx(&body(""), &[])).unwrap();
    assert!(plain.pages >= 3, "{plain:?}");
    let fit = r#"<sheetPr><pageSetUpPr fitToPage="1"/></sheetPr>"#;
    let (doc, fitted) = from_office("w.xlsx", &xlsx(&body(fit), &[])).unwrap();
    assert_eq!(fitted.pages, 1, "{fitted:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(Coluna0)") && text.contains("(Coluna7)") && text.contains(" cm\n"), "every column on the page, scaled");
    let scaled = r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><pageSetup scale="50"/>"#;
    let (doc, _) = from_office("s.xlsx", &xlsx(scaled, &[])).unwrap();
    assert!(content(&reopen(&doc), &pages(&reopen(&doc))[0]).starts_with("q\n0.5 0 0 0.5 "), "pageSetup scale");
}

// ── robustness and detection ───────────────────────────────────────────────────────────────

#[test]
fn hostile_and_unsupported_packages_are_refused_with_a_reason() {
    let err = |bytes: &[u8]| from_office("x.docx", bytes).unwrap_err().to_string();
    assert!(err(b"plain text").contains("not an Office Open XML package"));
    assert!(err(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]).contains("password-protected"));
    let no_main = build(&[("_rels/.rels", rels(&[]).as_bytes())]);
    assert!(err(&no_main).contains("no main document part"));
    let odd = build(&[("_rels/.rels", rels(&[("r", "officeDocument", "x.xml")]).as_bytes()), ("x.xml", b"<other/>")]);
    assert!(err(&odd).contains("not a Word, PowerPoint or Excel"));
    let dtd =
        build(&[("_rels/.rels", rels(&[("r", "officeDocument", "x.xml")]).as_bytes()), ("x.xml", b"<!DOCTYPE x [<!ENTITY a \"aaaa\">]><x>&a;</x>")]);
    assert!(err(&dtd).contains("malformed XML"), "DTDs are refused");
    // Deep nesting, cyclic styles, huge numbers and missing parts don't panic.
    let deep = format!("{}<w:p><w:r><w:t>fundo</w:t></w:r></w:p>{}", "<w:sdt><w:sdtContent>".repeat(200), "</w:sdtContent></w:sdt>".repeat(200));
    let (_, r) = from_office("deep.docx", &docx(&deep, &[], &[])).unwrap();
    assert_eq!(r.pages, 1);
    let huge = r#"<w:p><w:pPr><w:ind w:left="999999999999" w:hanging="-1e308"/><w:spacing w:line="NaN" w:before="1e300"/></w:pPr><w:r><w:rPr><w:sz w:val="1e30"/></w:rPr><w:t>x</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="0" w:h="-5"/><w:pgMar w:top="99999999" w:left="99999999" w:right="1" w:bottom="1"/></w:sectPr>"#;
    assert!(from_office("huge.docx", &docx(huge, &[], &[])).is_ok());
    let tbl = r#"<w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="99999"/><w:vMerge/></w:tcPr></w:tc></w:tr></w:tbl>"#;
    assert!(from_office("t.docx", &docx(tbl, &[], &[])).is_ok());
    let sheet = r#"<sheetData><row r="99999999999"><c r="ZZZZZZ1"><v>1e400</v></c><c r="A1" s="999"><v>NaN</v></c></row></sheetData><cols><col min="-5" max="99999999" width="1e300"/></cols>"#;
    assert!(from_office("h.xlsx", &xlsx(sheet, &[])).is_ok());
    // A table cell taller than a page is cut at the page bottom and the row moves on, rather
    // than starting page after page until the page limit.
    let tall = r#"<w:tbl><w:tblGrid><w:gridCol w:w="5000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="127000000"/><a:graphic><a:graphicData uri="urn:other"/></a:graphic></wp:inline></w:drawing></w:r></w:p><w:p><w:r><w:t>depois</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let (_, r) = from_office("tall.docx", &docx(tall, &[], &[])).unwrap();
    assert!(r.pages <= 3, "{r:?}");
    // Many merged ranges are capped, and each page only searches the ones that touch it.
    let merges: String = (1..=25_000).map(|i| format!(r#"<mergeCell ref="A{i}:B{i}"/>"#)).collect();
    let sheet = format!(r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><mergeCells>{merges}</mergeCells>"#);
    let started = std::time::Instant::now();
    let (_, r) = from_office("m.xlsx", &xlsx(&sheet, &[])).unwrap();
    assert!(r.notes.iter().any(|n| n.contains("merged ranges beyond")), "{r:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    let slide = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="x"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="NaN"><a:off x="1e308" y="-1e308"/><a:ext cx="1e308" cy="0"/></a:xfrm><a:custGeom><a:pathLst><a:path w="0" h="0"><a:moveTo><a:pt x="1" y="1"/></a:moveTo><a:arcTo wR="1" hR="1" stAng="0" swAng="1"/></a:path></a:pathLst></a:custGeom><a:solidFill><a:schemeClr val="nope"/></a:solidFill></p:spPr></p:sp>"#;
    assert!(from_office("p.pptx", &pptx(&[slide], &[])).is_ok());
}

#[test]
fn office_files_are_recognised_by_name_and_signature() {
    let d = docx("", &[], &[]);
    assert_eq!(source_kind("a.DOCX", &d), Some(SourceKind::Office));
    assert_eq!(source_kind("a.xlsx", &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]), Some(SourceKind::Office));
    assert_eq!(source_kind("a.zip", &d), None);
    assert_eq!(source_kind("notes.docx", b"just text"), None);
    // An empty document still converts to one blank page.
    let (doc, r) = from_office("vazio.docx", &d).unwrap();
    assert_eq!((r.pages, pages(&reopen(&doc)).len()), (1, 1));
}

#[test]
fn deeply_nested_word_content_is_reported_when_left_out() {
    let deep = format!("{}{}{}", "<w:sdt><w:sdtContent>".repeat(200), p("deep text"), "</w:sdtContent></w:sdt>".repeat(200));
    let (_, report) = from_office("deep.docx", &docx(&deep, &[], &[])).unwrap();
    assert!(report.notes.iter().any(|note| note.contains("nesting limit")), "{report:?}");
}

/// The text a page shows, decoded through each font's own encoding and `/ToUnicode` (whitespace
/// left out: words may be shown one by one).
fn shown_text(doc: &Document, page: &Dict) -> String {
    let fonts = page.get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
    let Object::Stream(s) = &*doc.resolve(page.get(b"Contents").unwrap()) else { panic!("no content") };
    let ops = s.decoded().unwrap();
    let (mut out, mut font, mut i) = (String::new(), None, 0);
    while let Some(&b) = ops.get(i) {
        match b {
            b'/' => {
                let end = ops[i + 1..].iter().position(|c| !c.is_ascii_alphanumeric()).map_or(ops.len(), |e| i + 1 + e);
                let name = ops[i + 1..end].to_vec();
                if let Some(f) = fonts.get(&name) {
                    font = Some(pdfcraft_fonts::pdf::Metrics::from_dict(doc, doc.resolve(f).as_dict().unwrap()));
                }
                i = end;
            }
            b'(' => {
                let mut bytes = Vec::new();
                i += 1;
                while let Some(&c) = ops.get(i) {
                    i += 1;
                    match c {
                        b')' => break,
                        b'\\' => {
                            bytes.push(ops[i]);
                            i += 1;
                        }
                        c => bytes.push(c),
                    }
                }
                out.push_str(&font.as_ref().expect("a font is set before text").decode(&bytes));
            }
            _ => i += 1,
        }
    }
    out.split_whitespace().collect()
}

/// Turkish letters (and the other Latin letters the standard faces carry beyond WinAnsi) are drawn,
/// searchable, in the run's own face; only scripts those faces lack are written as [U+XXXX].
#[test]
fn turkish_and_central_european_letters_are_drawn_not_escaped() {
    let turkish = "Ağaç İşçi ılık Şükrü Ğöl";
    let others = "Łódź Čeština Ősz Șțară";
    let body = format!(
        r#"<w:p><w:r><w:t>{turkish}</w:t></w:r></w:p><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>{others}</w:t></w:r><w:r><w:t xml:space="preserve"> Ωμέγα</w:t></w:r></w:p>"#
    );
    let (doc, report) = from_office("Türkçe.docx", &docx(&body, &[], &[])).unwrap();
    assert_eq!(report.escaped, 5, "only the Greek word is escaped: {report:?}");
    let doc = reopen(&doc);
    let page = &pages(&doc)[0];
    let text = shown_text(&doc, page);
    let expected: String = format!("{turkish} {others} [U+03A9][U+03BC][U+03AD][U+03B3][U+03B1]").split_whitespace().collect();
    assert_eq!(text, expected);
    // The extra letters come from Helvetica and Helvetica-Bold re-encoded, not from another font.
    let fonts = page.get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
    let latin: Vec<Dict> = fonts.iter().filter(|(k, _)| k.starts_with(b"L")).map(|(_, r)| doc.resolve(r).as_dict().unwrap().clone()).collect();
    let mut bases: Vec<&[u8]> = latin.iter().map(|f| f.name(b"BaseFont").unwrap()).collect();
    bases.sort();
    assert_eq!(bases, [&b"Helvetica"[..], b"Helvetica-Bold"]);
    for f in &latin {
        let enc = doc.resolve(f.get(b"Encoding").unwrap()).as_dict().unwrap().clone();
        let diffs = enc.get(b"Differences").unwrap().as_array().unwrap().clone();
        assert!(diffs.iter().any(|d| d.as_name() == Some(b"gbreve")) && diffs.iter().any(|d| d.as_name() == Some(b"dotlessi")));
        // ğ is as wide as g in the same face.
        let face = Std14::from_base_font(std::str::from_utf8(f.name(b"BaseFont").unwrap()).unwrap()).unwrap();
        let m = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, f);
        assert_eq!(m.width(u32::from(latin::code('ğ').unwrap())) * 1000.0, face.width(b'g'));
    }
}

/// PAGEREF (a table of contents' page number) keeps Word's cached result; only PAGE, NUMPAGES and
/// SECTIONPAGES are page fields, with or without a cached result.
#[test]
fn page_reference_fields_keep_their_cached_result() {
    let field = |instr: &str, cached: Option<&str>| {
        let result = cached.map_or(String::new(), |c| format!(r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{c}</w:t></w:r>"#));
        format!(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instr} </w:instrText></w:r>{result}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
        )
    };
    let body = [
        p("Contents"),
        field(r"PAGEREF _Toc123 \h", Some("77")),
        r#"<w:p><w:fldSimple w:instr=" PAGEREF _Toc456 \h "><w:r><w:t>88</w:t></w:r></w:fldSimple></w:p>"#.to_string(),
        field("PAGE", Some("9")),
        field("SECTIONPAGES", None),
        field("NUMPAGESX", Some("66")),
    ]
    .concat();
    let (doc, report) = from_office("toc.docx", &docx(&body, &[], &[])).unwrap();
    assert_eq!(report.pages, 1);
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(77)") && text.contains("(88)"), "the cross-reference page numbers Word cached: {text}");
    assert!(!text.contains("(9)"), "PAGE is the live page number, not its cached 9");
    assert_eq!(text.matches("(1)").count(), 2, "PAGE and SECTIONPAGES (no cached result) both show 1: {text}");
    assert!(text.contains("(66)"), "a field merely starting with NUMPAGES keeps its cached result");
}

/// The shape limit is per slide: a deck whose slides together pass it draws every slide, and a
/// slide past it is reported.
#[test]
fn the_shape_limit_applies_per_slide_and_is_reported() {
    let shape = |i: usize| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{i}" name="S"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr></p:sp>"#
        )
    };
    let label = |t: &str| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="L"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="500000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:txBody></p:sp>"#
        )
    };
    let busy: String = (0..12_000).map(shape).collect();
    let first = format!("{busy}{}", label("Primeiro"));
    let second = format!("{busy}{}", label("Segundo"));
    let (doc, report) = from_office("busy.pptx", &pptx(&[&first, &second], &[])).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("cut short")), "{report:?}");
    let doc = reopen(&doc);
    assert!(all_text(&doc).contains("(Segundo)"), "the second slide is drawn, not cut by the first slide's shapes");
    let crowded: String = (0..20_010).map(shape).collect();
    let (_, report) = from_office("crowded.pptx", &pptx(&[&crowded], &[])).unwrap();
    assert!(report.notes.iter().any(|n| n.contains("cut short")), "{report:?}");
}

/// Workbooks saved with the 1904 date system (older Excel for Mac) show their own dates, not dates
/// 66 years later.
#[test]
fn dates_in_1904_workbooks_are_not_shifted() {
    let sheet = |day: u32| {
        format!(
            r#"<cols><col min="1" max="3" width="14" customWidth="1"/></cols><sheetData><row r="1"><c r="A1" s="2"><v>0</v></c><c r="B1" s="2"><v>{day}</v></c><c r="C1" s="2"><v>61</v></c></row></sheetData>"#
        )
    };
    // 14 March 2019 is day 43538 in the 1900 system and day 42076 (1,462 fewer) in the 1904 one.
    let (doc, _) = from_office("1900.xlsx", &xlsx(&sheet(43_538), &[])).unwrap();
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(14/03/2019)") && text.contains("(01/03/1900)"), "the 1900 system, around its 29 February 1900: {text}");
    let (doc, _) = from_office("1904.xlsx", &xlsx_with(&sheet(42_076), &[], r#"<workbookPr date1904="1"/>"#)).unwrap();
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(01/01/1904)") && text.contains("(14/03/2019)") && text.contains("(02/03/1904)"), "{text}");
}

/// A sheet with one value in its last cell, columns as wide and rows as tall as Excel allows, has
/// billions of empty print pages: only the page with the value is looked at.
#[test]
fn a_sparse_sheet_prints_only_its_occupied_page_quickly() {
    let sheet = r#"<sheetFormatPr defaultColWidth="400" defaultRowHeight="409"/><sheetData><row r="1048576"><c r="XFD1048576" t="inlineStr"><is><t>Fim</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:B2"/></mergeCells>"#;
    let started = std::time::Instant::now();
    let (doc, report) = from_office("sparse.xlsx", &xlsx(sheet, &[])).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(report.pages, 1, "{report:?}");
    assert!(all_text(&reopen(&doc)).contains("(Fim)"));
}

/// A line break inside a cell starts a new line when the cell wraps, and is a space when it
/// doesn't; it is never dropped, joining the words on either side.
#[test]
fn line_breaks_in_cells_are_kept() {
    let sheet = r#"<cols><col min="1" max="2" width="30" customWidth="1"/></cols><sheetData><row r="1">
      <c r="A1" t="inlineStr" s="5"><is><t>Linha um
Linha dois</t></is></c>
      <c r="B1" t="inlineStr"><is><t>Sem
quebra</t></is></c></row></sheetData>"#;
    let (doc, _) = from_office("quebras.xlsx", &xlsx(sheet, &[])).unwrap();
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(Linha um)") && text.contains("(Linha dois)"), "{text}");
    assert!(text.contains("(Sem quebra)"), "{text}");
}

/// Every cell re-reads its number format, so a format of a million zeros (Excel stops at 255
/// characters) is shown as General, and reported, instead of padding each cell for minutes.
#[test]
fn an_oversized_number_format_is_shown_as_general_quickly() {
    let rows: String = (1..=200).map(|r| format!(r#"<row r="{r}"><c r="A{r}" s="1"><v>1234.5</v></c></row>"#)).collect();
    let sheet = format!(r#"<cols><col min="1" max="1" width="30" customWidth="1"/></cols><sheetData>{rows}</sheetData>"#);
    let started = std::time::Instant::now();
    let (doc, report) = from_office("zeros.xlsx", &xlsx_styled(&sheet, &[], "", &"0".repeat(1_000_000))).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert!(report.notes.iter().any(|n| n.contains("longer than Excel allows")), "{report:?}");
    assert!(all_text(&reopen(&doc)).contains("(1234.5)"));
    // Excel's longest format is still read.
    let (_, report) = from_office("long.xlsx", &xlsx_styled(&sheet, &[], "", &"0".repeat(255))).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("longer than Excel allows")), "{report:?}");
}

/// Gridlines and cell positions are found from running totals: a page whose band holds 200 000
/// hidden (zero-height) rows used to sum their heights again for every gridline, and drew a
/// gridline for each of them at the same place.
#[test]
fn gridlines_over_many_hidden_rows_are_drawn_once_and_quickly() {
    let hidden: String = (2..=200_000).map(|r| format!(r#"<row r="{r}" hidden="1"/>"#)).collect();
    let sheet = format!(
        r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Topo</t></is></c></row>{hidden}<row r="200001"><c r="A200001" t="inlineStr"><is><t>Base</t></is></c></row></sheetData><printOptions gridLines="1"/>"#
    );
    let started = std::time::Instant::now();
    let (doc, report) = from_office("hidden.xlsx", &xlsx(&sheet, &[])).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(report.pages, 1, "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(Topo)") && text.contains("(Base)"));
    // Three horizontal gridlines (above, between and below the two visible rows) and the
    // column's two vertical ones.
    assert_eq!(text.matches(" l S").count(), 5, "{text}");
}

/// A macro sheet is a relationship of its own type: its cells are not printed, and the report
/// says it was left out.
#[test]
fn macro_sheets_are_left_out_and_reported() {
    let main = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Vendas</t></is></c></row></sheetData>"#;
    let macro_sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>=HALT()</t></is></c></row></sheetData>"#;
    let (doc, report) = from_office("macros.xlsm", &xlsx(main, &[("Macro1", macro_sheet, false)])).unwrap();
    assert_eq!(report.pages, 1, "{report:?}");
    assert!(report.notes.iter().any(|n| n.contains("macro sheets, left out")), "{report:?}");
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(Vendas)") && !text.contains("HALT"), "{text}");
}

/// Placeholder lookups use an index built once per slide: a slide of 10 000 placeholders over a
/// master holding 50 000 used to walk the whole master for every one of them.
#[test]
fn placeholder_lookups_over_a_large_master_are_quick() {
    let sp = |id: usize, idx: usize| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="P"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="{idx}"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp>"#
        )
    };
    // Wrapped in an element the importer doesn't draw: the lookups still walked all of it.
    let master = format!("<p:custDataLst>{}</p:custDataLst>", (0..50_000).map(|i| sp(100 + i, 100_000 + i)).collect::<String>());
    let mut slide: String = (0..10_000).map(|i| sp(10 + i, 900_000 + i)).collect();
    slide.push_str(r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Titulo</a:t></a:r></a:p></p:txBody></p:sp>"#);
    let started = std::time::Instant::now();
    let (doc, report) = from_office("big.pptx", &pptx_with_master(&[&slide], &[], &master)).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(report.pages, 1, "{report:?}");
    let doc = reopen(&doc);
    // The title still takes the master title placeholder's 44 pt style.
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(Titulo)") && text.contains(" 44 Tf"), "{text}");
}

/// A layout, master or theme shared by every slide is parsed and indexed once: 200 slides over a
/// 100 000-shape master used to parse it 200 times.
#[test]
fn a_large_master_shared_by_many_slides_is_parsed_once() {
    let sp = |id: usize| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="P"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="{id}"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp>"#
        )
    };
    let master = format!("<p:custDataLst>{}</p:custDataLst>", (100..100_100).map(sp).collect::<String>());
    let slides: Vec<String> = (1..=200)
        .map(|i| {
            format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="T"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="500000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>Slide{i}</a:t></a:r></a:p></p:txBody></p:sp>"#
            )
        })
        .collect();
    let refs: Vec<&str> = slides.iter().map(String::as_str).collect();
    let started = std::time::Instant::now();
    let (doc, report) = from_office("deck.pptx", &pptx_with_master(&refs, &[], &master)).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(report.pages, 200, "{report:?}");
    let doc = reopen(&doc);
    assert!(content(&doc, &pages(&doc)[199]).contains("(Slide200)"));
}

/// Contextual spacing takes back the space after the previous paragraph only on the page that
/// space was left on: a paragraph that starts a new page starts at the top margin.
#[test]
fn contextual_spacing_after_a_page_break_starts_at_the_top() {
    let item = |t: &str, extra: &str| {
        format!(r#"<w:p><w:pPr>{extra}<w:spacing w:before="0" w:after="4000"/><w:contextualSpacing/></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#)
    };
    let body = format!("{}{}", item("Um", ""), item("Dois", "<w:pageBreakBefore/>"));
    let (doc, _) = from_office("ctx.docx", &docx(&body, &[], &[])).unwrap();
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert_eq!(pg.len(), 2);
    let y = |page: &Dict, w: &str| -> f64 {
        let text = content(&doc, page);
        let at = text.find(&format!("({w})")).unwrap();
        let tm = text[..at].rfind(" Tm").unwrap();
        text[..tm].split_whitespace().last().unwrap().parse().unwrap()
    };
    assert!((y(&pg[0], "Um") - y(&pg[1], "Dois")).abs() < 0.5, "{} vs {}", y(&pg[0], "Um"), y(&pg[1], "Dois"));
}

/// A footnote's own superscript text (m², a reference to a note in a source) is kept: only the
/// reference mark is added in front of it.
#[test]
fn superscript_text_in_a_footnote_is_kept() {
    let sup = r#"<w:rPr><w:vertAlign w:val="superscript"/></w:rPr>"#;
    let notes = format!(
        r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1"><w:p><w:r>{sup}<w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> Area em m</w:t></w:r><w:r>{sup}<w:t>2</w:t></w:r></w:p></w:footnote></w:footnotes>"#
    );
    let body = r#"<w:p><w:r><w:t>Texto</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p>"#;
    let (doc, _) =
        from_office("nota.docx", &docx(body, &[("word/footnotes.xml", notes.as_bytes())], &[("rIdF", "footnotes", "footnotes.xml")])).unwrap();
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(Area)") && text.contains("(2)"), "{text}");
}

/// Footnote marks are numbered with a running count, not by recounting every earlier reference,
/// so a document with very many references converts quickly.
#[test]
fn many_footnote_references_are_numbered_quickly() {
    let refs: String = (0..400).map(|_| r#"<w:r><w:footnoteReference w:id="9"/></w:r>"#).collect();
    let body: String = (0..500).map(|_| format!("<w:p>{refs}</w:p>")).collect();
    let started = std::time::Instant::now();
    let (doc, _) = from_office("refs.docx", &docx(&body, &[], &[])).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    // The marks run together with nothing between them; the last one ends the last word.
    assert!(all_text(&reopen(&doc)).contains("200000)"));
}

/// A slide whose part is missing or damaged is left out and reported, and the slides after it
/// are numbered as the slides actually drawn.
#[test]
fn a_damaged_slide_is_left_out_reported_and_not_counted() {
    let number = r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="N"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="9144000" cy="914400"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>Numero </a:t></a:r><a:fld id="{1}" type="slidenum"><a:t>9</a:t></a:fld></a:p></p:txBody></p:sp>"#;
    let (doc, report) = from_office("d.pptx", &pptx(&[number, "<broken", number], &[])).unwrap();
    assert_eq!(report.pages, 2, "{report:?}");
    assert!(report.notes.iter().any(|n| n.contains("missing or damaged")), "{report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert!(content(&doc, &pg[1]).contains("(2)"), "{}", content(&doc, &pg[1]));
}

/// Gothic faces are sans-serif even when their names hold a serif word ("Century", "Book").
#[test]
fn gothic_faces_are_sans_serif() {
    for name in ["Century Gothic", "Franklin Gothic Book", "MS Gothic", "Arial"] {
        assert_eq!(family_of(name), Fam::Sans, "{name}");
    }
    for name in ["Century Schoolbook", "Book Antiqua", "Times New Roman"] {
        assert_eq!(family_of(name), Fam::Serif, "{name}");
    }
}

/// A cell spanning two grid columns is followed by an `hMerge` placeholder for the second: the
/// cell after it starts at the third column, and the placeholder draws nothing.
#[test]
fn merged_table_cells_keep_later_cells_in_their_columns() {
    let cell =
        |attrs: &str, text: &str| format!(r#"<a:tc{attrs}><a:txBody><a:bodyPr/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>"#);
    let row = format!("{}{}{}", cell(r#" gridSpan="2""#, "Junto"), cell(r#" hMerge="1""#, "Oculto"), cell("", "C3"));
    let table = format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="2743200" cy="370840"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid><a:gridCol w="914400"/><a:gridCol w="914400"/><a:gridCol w="914400"/></a:tblGrid><a:tr h="370840">{row}</a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
    );
    let (doc, _) = from_office("t.pptx", &pptx(&[&table], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(!text.contains("(Oculto)"), "the placeholder's own text isn't drawn");
    let at = text.find("(C3)").unwrap_or_else(|| panic!("the third cell's text: {text}"));
    let tm = text[..at].rfind(" Tm").unwrap();
    let x: f64 = text[..tm].split_whitespace().rev().nth(1).unwrap().parse().unwrap();
    // The third column starts 144 pt in; its text sits just inside it, on one line.
    assert!((144.0..160.0).contains(&x), "x = {x}\n{text}");
}

/// A text box as Word 2010 and later write it: inside `mc:AlternateContent`, whose `Choice` is the
/// DrawingML shape and whose `Fallback` a VML copy (left empty here, so nesting stays small). Its
/// text is placed in the flow once.
fn text_box(inner: &str) -> String {
    format!(
        r#"<w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="wps"><w:drawing><wp:anchor><wp:extent cx="914400" cy="914400"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:txbx><w:txbxContent>{inner}</w:txbxContent></wps:txbx></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml"><v:textbox><w:txbxContent><w:p/></w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r>"#
    )
}

#[test]
fn text_boxes_in_alternate_content_are_placed_once() {
    let body = format!("<w:p>{}</w:p>", text_box(&p("Caixa")));
    let (doc, report) = from_office("tb.docx", &docx(&body, &[], &[])).unwrap();
    let text = all_text(&reopen(&doc));
    assert_eq!(text.matches("(Caixa)").count(), 1, "{text}");
    assert!(!report.notes.iter().any(|n| n.contains("text boxes")), "drawn in its box, not moved to the flow: {report:?}");
}

/// Text boxes nested in text boxes are each placed once: a box's own text, not every box below
/// it again, so nesting costs as much as the boxes, not twice as much per level.
#[test]
fn nested_text_boxes_are_placed_once_each_quickly() {
    let mut inner = p("Fundo");
    for level in 0..6 {
        inner = format!("<w:p><w:r><w:t>Nivel{level}</w:t></w:r>{}</w:p>", text_box(&inner));
    }
    let started = std::time::Instant::now();
    let (doc, _) = from_office("nested.docx", &docx(&inner, &[], &[])).unwrap_or_else(|e| panic!("{e}"));
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(all_text(&reopen(&doc)).matches("(Fundo)").count(), 1);
}

/// A note referenced again and again is placed once, so a long note can't be copied once per
/// reference.
#[test]
fn a_note_referenced_many_times_is_placed_once() {
    let body_paras: String = (0..200).map(|i| format!("<w:p><w:r><w:t>Corpo{i}</w:t></w:r></w:p>")).collect();
    let notes = format!(r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1">{body_paras}</w:footnote></w:footnotes>"#);
    let refs: String = (0..2000).map(|_| r#"<w:r><w:footnoteReference w:id="1"/></w:r>"#).collect();
    let body = format!("<w:p><w:r><w:t>Texto</w:t></w:r>{refs}</w:p>");
    let started = std::time::Instant::now();
    let (doc, _) =
        from_office("refs.docx", &docx(&body, &[("word/footnotes.xml", notes.as_bytes())], &[("rIdF", "footnotes", "footnotes.xml")])).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(all_text(&reopen(&doc)).matches("(Corpo199)").count(), 1);
}

/// The `x` of the first `Tm` before `word` in a content stream.
fn x_of(text: &str, word: &str) -> f64 {
    let at = text.find(&format!("({word})")).unwrap_or_else(|| panic!("{word} in {text}"));
    let tm = text[..at].rfind(" Tm").unwrap();
    text[..tm].split_whitespace().rev().nth(1).unwrap().parse().unwrap()
}

/// A slide placeholder is linked to the layout by index, but to the master by type: a content
/// placeholder with index 2 takes the master's body style, not its date placeholder's (also 2).
#[test]
fn content_placeholders_inherit_the_master_body_not_its_date() {
    let date = r#"<p:sp><p:nvSpPr><p:cNvPr id="7" name="Date"/><p:cNvSpPr/><p:nvPr><p:ph type="dt" idx="2"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="6356350"/><a:ext cx="2133600" cy="365125"/></a:xfrm></p:spPr><p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle><a:lvl1pPr algn="r"/></a:lstStyle><a:p/></p:txBody></p:sp>"#;
    let slide = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Content 2"/><p:cNvSpPr/><p:nvPr><p:ph idx="2"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="9144000" cy="1828800"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>Conteudo</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let (doc, _) = from_office("ph.pptx", &pptx_with_master(&[slide], &[], date)).unwrap();
    let doc = reopen(&doc);
    let x = x_of(&content(&doc, &pages(&doc)[0]), "Conteudo");
    assert!(x < 100.0, "left-aligned like the body, not right-aligned like the date: x = {x}");
}

/// Slide number, date and footer placeholders use the master's other-text style: no body bullets.
#[test]
fn slide_number_placeholders_have_no_bullets() {
    let slide = r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="Number"/><p:cNvSpPr/><p:nvPr><p:ph type="sldNum" idx="12"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="6553200" y="6356350"/><a:ext cx="2133600" cy="365125"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:fld id="{1}" type="slidenum"><a:t>9</a:t></a:fld></a:p></p:txBody></p:sp>"#;
    let (doc, _) = from_office("n.pptx", &pptx(&[slide], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(1)"), "{text}");
    assert!(!text.contains('\u{95}'), "no bullet before the number: {text}");
}

/// Table cells keep their line breaks and fields.
#[test]
fn table_cells_keep_line_breaks_and_slide_numbers() {
    let cell = r#"<a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Linha1</a:t></a:r><a:br/><a:r><a:t>Linha2</a:t></a:r><a:r><a:t> slide </a:t></a:r><a:fld id="{1}" type="slidenum"><a:t>9</a:t></a:fld></a:p></a:txBody><a:tcPr/></a:tc>"#;
    let table = format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="6000000" cy="370840"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid><a:gridCol w="6000000"/></a:tblGrid><a:tr h="370840">{cell}</a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
    );
    let (doc, _) = from_office("tc.pptx", &pptx(&[&table], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(Linha1)") && text.contains("(Linha2)") && !text.contains("Linha1Linha2"), "{text}");
    assert!(text.contains("(1)"), "the slide number field: {text}");
}

/// `horizontal="general"` (LibreOffice writes it on every style) is the default alignment.
#[test]
fn general_horizontal_alignment_is_the_default() {
    let sheet = r#"<sheetData><row r="1"><c r="A1"><v>7</v></c></row><row r="2"><c r="A2" s="6"><v>7</v></c></row></sheetData>"#;
    let (doc, _) = from_office("g.xlsx", &xlsx(sheet, &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    let xs: Vec<f64> = text
        .match_indices("(7)")
        .map(|(at, _)| {
            let tm = text[..at].rfind(" Tm").unwrap();
            text[..tm].split_whitespace().rev().nth(1).unwrap().parse().unwrap()
        })
        .collect();
    assert_eq!(xs.len(), 2, "{text}");
    assert!((xs[0] - xs[1]).abs() < 0.01, "both right-aligned: {xs:?}");
}

/// A stored column width already holds Excel's padding: the default column, stored as 9.140625,
/// is 64 px wide, as wide as an unstated one.
#[test]
fn stored_column_widths_are_not_padded_twice() {
    let x_b = |cols: &str| {
        let sheet = format!(
            r#"{cols}<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c><c r="B1" t="inlineStr"><is><t>Bcol</t></is></c></row></sheetData>"#
        );
        let (doc, _) = from_office("w.xlsx", &xlsx(&sheet, &[])).unwrap();
        let doc = reopen(&doc);
        x_of(&content(&doc, &pages(&doc)[0]), "Bcol")
    };
    let stated = x_b(r#"<cols><col min="1" max="1" width="9.140625" customWidth="1"/></cols>"#);
    let unstated = x_b("");
    assert!((stated - unstated).abs() < 0.01, "{stated} vs {unstated}");
}

/// Names with "Unicode" in them are proportional faces, not code fonts.
#[test]
fn unicode_faces_are_not_monospaced() {
    for name in ["Arial Unicode MS", "Lucida Sans Unicode"] {
        assert_eq!(family_of(name), Fam::Sans, "{name}");
    }
    for name in ["Courier New", "Consolas", "Fira Code", "Source Code Pro"] {
        assert_eq!(family_of(name), Fam::Mono, "{name}");
    }
}

/// A long shared string referenced by many cells is copied into each: past a cap on a sheet's
/// cell text, the rest is left out and reported instead of exhausting memory.
#[test]
fn cell_text_is_capped_per_sheet() {
    let long = "x".repeat(100_000);
    let rows: String = (1..=1000).map(|r| format!(r#"<row r="{r}"><c r="A{r}" t="s"><v>0</v></c></row>"#)).collect();
    let sheet = format!("<sheetData>{rows}</sheetData>");
    let (_, report) = from_office("big.xlsx", &xlsx_full(&sheet, &[], "", "0", Some(&format!("<si><t>{long}</t></si>")), &[])).unwrap();
    assert!(report.notes.iter().any(|n| n.contains("cell text")), "{report:?}");
}

/// Finding the merge a cell is in is bounded: a page dense with tiny rows and merges stops
/// looking past a budget, draws the rest unmerged and says so, instead of scanning every merge
/// for every cell.
#[test]
fn merge_lookups_on_a_dense_page_are_bounded() {
    let rows: String = (1..=20_000)
        .map(|r| format!(r#"<row r="{r}" ht="0.1" customHeight="1"><c r="A{r}" t="inlineStr"><is><t>a</t></is></c><c r="C{r}" t="inlineStr"><is><t>c</t></is></c></row>"#))
        .collect();
    let merges: String = (1..=20_000).map(|r| format!(r#"<mergeCell ref="A{r}:B{r}"/>"#)).collect();
    let sheet = format!(r#"<sheetData>{rows}</sheetData><mergeCells count="20000">{merges}</mergeCells>"#);
    let started = std::time::Instant::now();
    let (_, report) = from_office("dense.xlsx", &xlsx(&sheet, &[])).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert!(report.notes.iter().any(|n| n.contains("drawn unmerged")), "{report:?}");
}

// ── Regressions found by review: limits, correctness of presets, numbering and columns ──────────

/// A numbering level's text is copied into every paragraph it numbers: it is capped so a hostile
/// level can't be multiplied by the paragraph count.
#[test]
fn long_numbering_text_is_capped() {
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="{}"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#,
        "Q".repeat(100_000)
    );
    let numbered = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r></w:p>"#;
    let body = numbered.repeat(5);
    let started = std::time::Instant::now();
    let (doc, _) = from_office("n.docx", &docx_numbered(&body, &[], &[], Some(&numbering))).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    let drawn = all_text(&reopen(&doc)).matches('Q').count();
    assert!(drawn <= 5 * 64, "each label is at most 64 characters, but {drawn} were drawn for 5 paragraphs");
}

/// Looking for the cell below a vertically merged cell must not scan the whole next row for every
/// cell of this one.
#[test]
fn vmerge_lookup_is_not_quadratic() {
    let row = format!("<w:tr>{}</w:tr>", "<w:tc/>".repeat(240_000));
    let body = format!("<w:tbl><w:tblPr/>{row}{row}</w:tbl>");
    let started = std::time::Instant::now();
    let result = from_office("wide.docx", &docx(&body, &[], &[]));
    let took = started.elapsed();
    assert!(result.is_ok(), "{:?}", result.err());
    assert!(took < std::time::Duration::from_secs(20), "converting took {took:?}");
}

/// "Monotype" is a foundry, not a code face: only real code faces are monospaced.
#[test]
fn monotype_faces_are_not_monospaced() {
    for name in ["Monotype Corsiva", "Monotype Sorts"] {
        assert_ne!(family_of(name), Fam::Mono, "{name}");
    }
    for name in ["Courier New", "Consolas", "Fira Code"] {
        assert_eq!(family_of(name), Fam::Mono, "{name}");
    }
}

/// A `frame` or `donut` has a hole. Drawn as a filled rectangle or ellipse it hides what is
/// underneath, so it is either drawn with its hole (even-odd fill) or reported as approximated.
#[test]
fn frame_and_donut_are_not_solid() {
    let under = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Under"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="1000000" y="1000000"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>Underneath</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let shape = |id: u32, prst: &str, x: u32| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="S{id}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="500000"/><a:ext cx="3000000" cy="2000000"/></a:xfrm><a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></p:spPr></p:sp>"#
        )
    };
    let slide = format!("{under}{}{}", shape(3, "donut", 500_000), shape(4, "frame", 4_500_000));
    let (doc, report) = from_office("hole.pptx", &pptx(&[&slide], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    let noted = report.notes.iter().any(|n| n.contains("approximat"));
    let even_odd = text.contains("f*") || text.contains("B*");
    assert!(noted || even_odd, "frame and donut are drawn solid without a note or an even-odd fill: {report:?}\n{text}");
}

fn numbered_slide() -> String {
    r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="Number"/><p:cNvSpPr/><p:nvPr><p:ph type="sldNum" idx="12"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="6553200" y="6356350"/><a:ext cx="2133600" cy="365125"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:fld id="{1}" type="slidenum"><a:t>9</a:t></a:fld></a:p></p:txBody></p:sp>"#
        .to_owned()
}

/// Slide numbers follow the deck: they start at `firstSlideNum`, and a hidden slide still has its
/// number, so the slides after it are not renumbered.
#[test]
fn slide_numbers_follow_the_deck() {
    let slide = numbered_slide();
    // A: the first slide is numbered 0.
    let (doc, _) = from_office("a.pptx", &pptx_full(&[&slide], &[], "", r#" firstSlideNum="0""#)).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(0)"), "firstSlideNum=0 numbers the first slide 0: {text}");
    // B: slide 2 is hidden; the third slide keeps the number 3.
    let (doc, _) = from_office("b.pptx", &pptx(&[&slide, &slide, &slide], &[false, true, false])).unwrap();
    let doc = reopen(&doc);
    let all = pages(&doc);
    assert_eq!(all.len(), 2);
    let second = content(&doc, &all[1]);
    assert!(second.contains("(3)"), "the slide after a hidden one keeps its number: {second}");
}

/// Excel's `#,###` shows nothing for zero (the integer part is optional digits only).
#[test]
fn accounting_zero_has_no_digit() {
    let sheet = r#"<sheetData><row r="1"><c r="A1" s="1"><v>0</v></c></row></sheetData>"#;
    let (doc, _) = from_office("z.xlsx", &xlsx_styled(sheet, &[], "", "#,###")).unwrap();
    let doc = reopen(&doc);
    let text = all_text(&doc);
    assert!(!text.contains("(0)"), "zero under #,### is drawn empty: {text}");
}

/// Every `<col>` element costs a bounded amount, however many there are.
#[test]
fn col_elements_share_a_work_budget() {
    let cols = r#"<col min="1" max="16384" hidden="1"/>"#.repeat(400_000);
    let sheet = format!(r#"<cols>{cols}</cols><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c></row></sheetData>"#);
    let started = std::time::Instant::now();
    let _ = from_office("cols.xlsx", &xlsx(&sheet, &[]));
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(20), "converting took {took:?}");
}

/// A column range reaching the last column hides all of it, not only its first 4097 columns.
#[test]
fn col_ranges_hide_every_column_they_name() {
    let sheet = r#"<cols><col min="2" max="16384" hidden="1"/></cols><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Acell</t></is></c><c r="FZZ1" t="inlineStr"><is><t>Farcell</t></is></c></row></sheetData>"#;
    let (doc, _) = from_office("hide.xlsx", &xlsx(sheet, &[])).unwrap();
    let doc = reopen(&doc);
    let text = all_text(&doc);
    assert!(text.contains("(Acell)"), "{text}");
    assert!(!text.contains("(Farcell)"), "column FZZ (4628) is inside the hidden range: {text}");
}

// ── layout fidelity (tabs, borders, table styles, headers, floats, EMF, Excel print setup) ──

/// The y of the `Tm` that draws `word`.
fn y_of(text: &str, word: &str) -> f64 {
    let at = text.find(&format!("({word})")).unwrap_or_else(|| panic!("{word} in {text}"));
    let tm = text[..at].rfind(" Tm").unwrap();
    text[..tm].split_whitespace().next_back().unwrap().parse().unwrap()
}

/// The fill colour (`r g b rg`) set by the `BT` that draws `word`.
fn colour_of(text: &str, word: &str) -> String {
    let at = text.find(&format!("({word})")).unwrap_or_else(|| panic!("{word} in {text}"));
    let bt = text[..at].rfind("BT ").unwrap();
    text[bt + 3..].split(" rg").next().unwrap().to_owned()
}

#[test]
fn word_fields_number_pages_by_section_follow_headings_and_show_today() {
    let field = |instr: &str, cached: &str| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{instr}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{cached}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    let footer = format!(
        r#"<w:ftr xmlns:w="{W}"><w:p><w:r><w:t xml:space="preserve">Pg </w:t></w:r><w:fldSimple w:instr=" PAGE "><w:r><w:t>9</w:t></w:r></w:fldSimple><w:r><w:t xml:space="preserve"> de </w:t></w:r>{}<w:r><w:t xml:space="preserve"> n</w:t></w:r>{}<w:r><w:t xml:space="preserve"> em </w:t></w:r>{}</w:p></w:ftr>"#,
        field(" SECTIONPAGES ", "9"),
        field(r" PAGE \* Arabic ", "9"),
        field(r#" DATE \@ "yyyy-MM-dd" "#, "1999-01-01"),
    );
    let header = format!(
        r#"<w:hdr xmlns:w="{W}"><w:p><w:r><w:t xml:space="preserve">Cap: </w:t></w:r>{}</w:p></w:hdr>"#,
        field(r#" STYLEREF "Heading 1" \* MERGEFORMAT "#, "Velho")
    );
    let body = r#"<w:p><w:r><w:t>Capa</w:t></w:r></w:p><w:p><w:pPr><w:sectPr><w:footerReference w:type="default" r:id="rIdF"/></w:sectPr></w:pPr></w:p>
        <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Alfa</w:t></w:r></w:p><w:p><w:r><w:t>Texto</w:t><w:br w:type="page"/><w:t xml:space="preserve">Sem titulo p</w:t></w:r><w:fldSimple w:instr=" PAGE "><w:r><w:t>9</w:t></w:r></w:fldSimple><w:r><w:t xml:space="preserve"> de </w:t></w:r><w:fldSimple w:instr=" NUMPAGES "><w:r><w:t>9</w:t></w:r></w:fldSimple><w:r><w:t xml:space="preserve"> s</w:t></w:r><w:fldSimple w:instr=" SECTIONPAGES "><w:r><w:t>9</w:t></w:r></w:fldSimple><w:r><w:br w:type="page"/></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Beta</w:t></w:r></w:p>
        <w:sectPr><w:headerReference w:type="default" r:id="rIdH"/><w:footerReference w:type="default" r:id="rIdF"/><w:pgNumType w:fmt="lowerRoman" w:start="1"/></w:sectPr>"#;
    let (doc, report) = from_office(
        "campos.docx",
        &docx(
            body,
            &[("word/header1.xml", header.as_bytes()), ("word/footer1.xml", footer.as_bytes())],
            &[("rIdH", "header", "header1.xml"), ("rIdF", "footer", "footer1.xml")],
        ),
    )
    .unwrap();
    assert_eq!(report.pages, 4, "{report:?}");
    let doc = reopen(&doc);
    // A page's text, its strings joined without spaces.
    let page_text = |i: usize| -> String {
        let text = content(&doc, &pages(&doc)[i]);
        text.split('(').skip(1).filter_map(|s| s.split(") Tj").next()).collect::<String>().replace(' ', "")
    };
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let expect = [("", "Pg1de1n1"), ("Cap:Alfa", "Pgide3n1"), ("Cap:Alfa", "Pgiide3n2"), ("Cap:Beta", "Pgiiide3n3")];
    for (i, (head, foot)) in expect.iter().enumerate() {
        let t = page_text(i);
        assert!(t.contains(&format!("{foot}em{today}")), "page {}: {foot} and today in {t}", i + 1);
        assert!(head.is_empty() || t.contains(head), "page {}: {head} in {t}", i + 1);
        assert!(!t.contains("Velho") && !t.contains("1999"), "cached results replaced: {t}");
    }
    // Page fields in the body show the page they are on and the document's and section's counts.
    assert!(page_text(2).contains("Semtitulopiide4s3"), "{}", page_text(2));
}

/// The link annotations of a page: each one's URI, or the index of the page its destination is on.
fn page_links(doc: &Document, page: &Dict) -> Vec<Result<String, usize>> {
    let kids: Vec<pdfcraft_cos::ObjRef> = {
        let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
        doc.get(pages).as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap().iter().filter_map(|k| k.as_ref()).collect()
    };
    let Some(annots) = page.get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()) else { return Vec::new() };
    annots
        .iter()
        .map(|a| {
            let a = doc.resolve(a).as_dict().cloned().unwrap();
            assert_eq!(a.get(b"Subtype").and_then(|s| s.as_name()), Some(&b"Link"[..]));
            match a.get(b"A").and_then(|x| doc.resolve(x).as_dict().cloned()) {
                Some(action) => Ok(String::from_utf8_lossy(&action.get(b"URI").and_then(|u| u.as_string()).unwrap().bytes).into_owned()),
                None => {
                    let dest = a.get(b"Dest").unwrap().as_array().unwrap().clone();
                    Err(kids.iter().position(|k| Some(*k) == dest[0].as_ref()).unwrap())
                }
            }
        })
        .collect()
}

#[test]
fn word_links_go_to_their_address_or_bookmark_and_headings_become_bookmarks() {
    let body = r#"<w:p><w:hyperlink r:id="rIdL"><w:r><w:t xml:space="preserve">Site da empresa</w:t></w:r></w:hyperlink></w:p>
        <w:p><w:hyperlink w:anchor="Alvo"><w:r><w:t>Ir ao alvo</w:t></w:r></w:hyperlink></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> REF Alvo \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Capitulo</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>
        <w:p><w:hyperlink r:id="rIdJ"><w:r><w:t>Perigoso</w:t></w:r></w:hyperlink></w:p>
        <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Primeiro</w:t></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="Heading1"/><w:pageBreakBefore/></w:pPr><w:bookmarkStart w:id="1" w:name="Alvo"/><w:r><w:t>Segundo</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>"#;
    let (doc, _) = from_office(
        "links.docx",
        &docx(body, &[], &[("rIdL", "hyperlink", "https://example.com/a?b=1"), ("rIdJ", "hyperlink", "javascript:alert(1)")]),
    )
    .unwrap();
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert_eq!(pg.len(), 2);
    let links = page_links(&doc, &pg[0]);
    assert_eq!(links, vec![Ok("https://example.com/a?b=1".to_owned()), Err(1), Err(1)], "one area per link, the script left out");
    // Headings are bookmarks, in order, going to their pages.
    let root = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let outlines = doc.resolve(root.get(b"Outlines").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(outlines.get(b"Count").and_then(|c| c.as_f64()), Some(2.0));
    let first = doc.resolve(outlines.get(b"First").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(first.get(b"Title").and_then(|t| t.as_string()).map(|t| t.to_text()), Some("Primeiro".to_owned()));
    let second = doc.resolve(first.get(b"Next").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(second.get(b"Title").and_then(|t| t.as_string()).map(|t| t.to_text()), Some("Segundo".to_owned()));
}

#[test]
fn excel_hyperlinks_go_to_their_address_or_sheet() {
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Site</t></is></c><c r="B1" t="inlineStr"><is><t>Resumo</t></is></c></row></sheetData>
      <hyperlinks><hyperlink ref="A1" r:id="rIdL"/><hyperlink ref="B1" location="'Folha 2'!A1"/></hyperlinks>"#;
    let rels_xml = sheet_rels(&[("rIdL", "hyperlink", "https://example.com/")]);
    let second = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Dois</t></is></c></row></sheetData>"#;
    let extra: [(&str, &[u8]); 1] = [("xl/worksheets/_rels/sheet1.xml.rels", rels_xml.as_bytes())];
    let (doc, _) = from_office("l.xlsx", &xlsx_full(sheet, &[("Folha 2", second, false)], "", "0", None, &extra)).unwrap();
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert_eq!(page_links(&doc, &pg[0]), vec![Ok("https://example.com/".to_owned()), Err(1)]);
}

#[test]
fn widows_orphans_and_kept_lines_move_to_the_next_page() {
    // Twelve-point lines, six to a page: where each paragraph's lines land.
    let line = |t: &str, ppr: &str| {
        format!(r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="exact"/>{ppr}</w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#)
    };
    let lines = |tag: &str, n: usize, ppr: &str| {
        let runs: Vec<String> = (1..=n).map(|i| format!("<w:t>{tag}{i}</w:t>")).collect();
        format!(
            r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="exact"/>{ppr}</w:pPr><w:r>{}</w:r></w:p>"#,
            runs.join("<w:br/>")
        )
    };
    let sect = r#"<w:sectPr><w:pgSz w:w="6000" w:h="2880"/><w:pgMar w:top="720" w:right="720" w:bottom="720" w:left="720" w:header="0" w:footer="0"/></w:sectPr>"#;
    let page_of = |body: String, word: &str| -> usize {
        let (doc, _) = from_office("w.docx", &docx(&format!("{body}{sect}"), &[], &[])).unwrap();
        let doc = reopen(&doc);
        pages(&doc).iter().position(|p| content(&doc, p).contains(&format!("({word})"))).unwrap()
    };
    let filler = |n: usize| (0..n).map(|i| line(&format!("F{i}"), "")).collect::<String>();
    // A widow: three of four lines fit; the third moves with the last.
    assert_eq!(page_of(format!("{}{}", filler(3), lines("W", 4, "")), "W3"), 1);
    assert_eq!(page_of(format!("{}{}", filler(3), lines("W", 4, r#"<w:widowControl w:val="0"/>"#)), "W3"), 0, "without control it stays");
    // An orphan: only the first line fits; the paragraph moves.
    assert_eq!(page_of(format!("{}{}", filler(5), lines("O", 3, "")), "O1"), 1);
    // Keep lines together: two of three fit; all three move.
    assert_eq!(page_of(format!("{}{}", filler(4), lines("K", 3, r#"<w:widowControl w:val="0"/><w:keepLines/>"#)), "K1"), 1);
    assert_eq!(page_of(format!("{}{}", filler(4), lines("K", 3, r#"<w:widowControl w:val="0"/>"#)), "K1"), 0);
}

#[test]
fn page_colour_borders_line_numbers_and_drop_caps() {
    let long = "palavra ".repeat(40);
    let body = format!(
        r#"<w:background w:color="FFFFCC"/><w:p><w:pPr><w:framePr w:dropCap="drop" w:lines="3" w:wrap="around" w:vAnchor="text" w:hAnchor="text"/><w:spacing w:after="0" w:line="720" w:lineRule="exact"/></w:pPr><w:r><w:rPr><w:sz w:val="96"/></w:rPr><w:t>Q</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">uem {long}</w:t></w:r></w:p><w:p><w:pPr><w:suppressLineNumbers/></w:pPr><w:r><w:t>Semnumero</w:t></w:r></w:p>
        <w:sectPr><w:pgBorders w:offsetFrom="page"><w:top w:val="single" w:sz="8" w:space="24" w:color="FF0000"/><w:left w:val="dotted" w:sz="8" w:space="24" w:color="FF0000"/><w:bottom w:val="single" w:sz="8" w:space="24" w:color="FF0000"/><w:right w:val="single" w:sz="8" w:space="24" w:color="FF0000"/></w:pgBorders><w:lnNumType w:countBy="1" w:restart="newPage"/></w:sectPr>"#
    );
    let (doc, _) = from_office("p.docx", &docx(&body, &[], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.starts_with("q 1 1 0.8 rg 0 0 612 792 re f Q"), "the page colour under everything: {}", &text[..80.min(text.len())]);
    assert!(text.contains("1 0 0 RG 1 w 0 J 24.5 767.5 m 587.5 767.5 l S"), "the top border a third of an inch in: {text}");
    assert!(text.contains("[1 2] 0 d"), "the dotted left border");
    // Line numbers left of the text, one per line, not for the suppressed paragraph.
    assert!(x_of(&text, "1") < 72.0 && x_of(&text, "2") < 72.0, "line numbers in the margin: {text}");
    let numbered: Vec<f64> = (1..30).filter(|i| text.contains(&format!("({i})"))).map(|i| y_of(&text, &i.to_string())).collect();
    assert!(numbered.len() >= 3, "{text}");
    assert!(numbered.iter().all(|y| (y - y_of(&text, "Semnumero")).abs() > 1.0), "the suppressed paragraph isn't numbered");
    // The drop cap sits left of the paragraph's first three lines, its text beside it.
    let (q, uem) = (x_of(&text, "Q"), x_of(&text, "uem"));
    assert!(uem > q + 20.0, "the text wraps beside the letter: {q} {uem}");
    assert!(y_of(&text, "Q") < y_of(&text, "uem"), "the letter drops below the first line");
}

#[test]
fn tables_floating_in_the_text_have_the_text_wrap_beside_them() {
    let words: String = (0..120).map(|i| format!("w{i} ")).collect();
    let body = format!(
        r#"<w:tbl><w:tblPr><w:tblpPr w:leftFromText="180" w:rightFromText="180" w:vertAnchor="text" w:tblpXSpec="right" w:tblpY="1"/><w:tblStyle w:val="TableGrid"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>Flutua</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>Ainda</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t xml:space="preserve">{words}</w:t></w:r></w:p>"#
    );
    let (doc, _) = from_office("ft.docx", &docx(&body, &[], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    let (tx, ty, bottom) = (x_of(&text, "Flutua"), y_of(&text, "Flutua"), y_of(&text, "Ainda"));
    let placed: Vec<(f64, f64)> =
        (0..120).filter(|i| text.contains(&format!("(w{i})"))).map(|i| (x_of(&text, &format!("w{i}")), y_of(&text, &format!("w{i}")))).collect();
    assert!(placed.iter().filter(|(_, y)| *y > bottom - 2.0).all(|(x, _)| *x < tx - 9.0), "beside the table the lines stop short of it: {text}");
    assert!(placed.iter().any(|(x, y)| *y < bottom - 20.0 && *x > tx), "below it they run the full width");
    assert!(placed.first().is_some_and(|(_, y)| (y - ty).abs() < 3.0), "the table sits beside the first line");
}

#[test]
fn tracked_changes_print_as_the_final_text_and_vml_boxes_and_turned_text_are_drawn() {
    let body = format!(
        r##"<w:p><w:r><w:t xml:space="preserve">Fica </w:t></w:r><w:del w:id="1" w:author="A"><w:r><w:delText>Apagado</w:delText></w:r></w:del><w:ins w:id="2" w:author="A"><w:r><w:t>Novo</w:t></w:r></w:ins><w:moveFrom w:id="3" w:author="A"><w:r><w:t>Movido</w:t></w:r></w:moveFrom></w:p>
        <w:p><w:r><w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml" style="position:absolute;margin-left:200pt;margin-top:0;width:100pt;height:50pt" fillcolor="#FF0000" strokecolor="blue"><v:textbox><w:txbxContent><w:p><w:r><w:t>Caixa</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></w:r><w:r><w:t>Corpo</w:t></w:r></w:p>
        <w:tbl><w:tblGrid><w:gridCol w:w="800"/><w:gridCol w:w="3000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:textDirection w:val="btLr"/></w:tcPr><w:p><w:r><w:t>Subindo</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Normal</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
        <w:p><w:r><w:drawing><wp:inline><wp:extent cx="635000" cy="1270000"/><a:graphic><a:graphicData uri="{WPS}"><wps:wsp xmlns:wps="{WPS}"><wps:spPr><a:prstGeom prst="rect"/></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>Descendo</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr vert="vert"/></wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"##
    );
    let (doc, report) = from_office("tc.docx", &docx(&body, &[], &[])).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("text boxes")), "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(
        text.contains("(Novo)") && !text.contains("Apagado") && !text.contains("Movido"),
        "insertions kept, deletions and moves away gone: {text}"
    );
    assert!(text.contains("1 0 0 rg") && text.contains("0 0 1 RG"), "the VML box's fill and outline");
    assert!(x_of(&text, "Caixa") > 72.0 + 200.0, "the VML box where its style puts it");
    let before = |w: &str| {
        text[..text.find(&format!("({w})")).unwrap()].rfind(" cm").map(|i| text[..i].rsplit('\n').next().unwrap_or("").to_owned()).unwrap_or_default()
    };
    assert!(before("Subindo").starts_with("0 1 -1 0"), "the cell's text reads upwards: {}", before("Subindo"));
    assert!(before("Descendo").starts_with("0 -1 1 0"), "the box's text reads downwards: {}", before("Descendo"));
}

#[test]
fn right_centre_and_decimal_tab_stops_place_text_at_the_stop() {
    let toc = r#"<w:p><w:pPr><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9000"/></w:tabs></w:pPr><w:r><w:t>Capitulo</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>42</w:t></w:r></w:p>"#;
    let more = r#"<w:p><w:pPr><w:tabs><w:tab w:val="center" w:pos="4500"/><w:tab w:val="decimal" w:pos="8000"/></w:tabs></w:pPr><w:r><w:t>a</w:t><w:tab/><w:t>Meio</w:t><w:tab/><w:t>12.5</w:t></w:r></w:p>"#;
    let (doc, _) = from_office("tabs.docx", &docx(&format!("{toc}{more}"), &[], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    let w = |s: &str| pdfcraft_fonts::Std14::Helvetica.text_width(s, 11.0);
    // Stops are measured from the left margin (72 pt here).
    let end = x_of(&text, "42") + w("42");
    assert!((end - (72.0 + 450.0)).abs() < 0.5, "the page number ends at the right stop: {end}");
    assert_eq!(y_of(&text, "42"), y_of(&text, "Capitulo"), "on the entry's line");
    assert!(text.contains("(....."), "a dot leader fills the gap: {text}");
    let mid = x_of(&text, "Meio") + w("Meio") / 2.0;
    assert!((mid - (72.0 + 225.0)).abs() < 0.5, "centred on the centre stop: {mid}");
    let point = x_of(&text, "12.5") + w("12");
    assert!((point - (72.0 + 400.0)).abs() < 0.5, "the decimal point on the decimal stop: {point}");
}

#[test]
fn table_of_contents_links_print_without_the_hyperlink_style() {
    let link =
        |t: &str| format!(r#"<w:hyperlink w:anchor="_Toc1"><w:r><w:rPr><w:rStyle w:val="Hyperlink"/></w:rPr><w:t>{t}</w:t></w:r></w:hyperlink>"#);
    let body = format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}</w:p>
        <w:p>{}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p><w:p>{}</w:p>"#,
        link("Entrada"),
        link("Segunda"),
        link("Fora")
    );
    let (doc, _) = from_office("toc.docx", &docx(&body, &[], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert_eq!(colour_of(&text, "Entrada"), "0 0 0", "in the field's result, across paragraphs");
    assert_eq!(colour_of(&text, "Segunda"), "0 0 0");
    assert_eq!(colour_of(&text, "Fora"), "0.02 0.388 0.757", "a link after the field keeps its style");
}

#[test]
fn paragraphs_with_the_same_borders_share_one_box() {
    let bdr = r#"<w:pBdr><w:top w:val="single" w:sz="8" w:space="4" w:color="FF0000"/><w:left w:val="single" w:sz="8" w:space="4" w:color="FF0000"/><w:bottom w:val="single" w:sz="8" w:space="4" w:color="FF0000"/><w:right w:val="single" w:sz="8" w:space="4" w:color="FF0000"/></w:pBdr>"#;
    let para = |t: &str| format!(r#"<w:p><w:pPr>{bdr}</w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#);
    let double =
        r#"<w:p><w:pPr><w:pBdr><w:top w:val="double" w:sz="4" w:space="1" w:color="0000FF"/></w:pBdr></w:pPr><w:r><w:t>Duplo</w:t></w:r></w:p>"#;
    let (doc, _) = from_office("b.docx", &docx(&format!("{}{}{double}", para("Um"), para("Dois")), &[], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    let lines = |colour: &str| -> Vec<(f64, f64, f64, f64)> {
        text.lines()
            .filter_map(|l| {
                let rest = l.strip_prefix(&format!("{colour} RG "))?;
                // `w 0 J x1 y1 m x2 y2 l S`: the width and cap style come first.
                let v: Vec<f64> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                (v.len() >= 6).then(|| (v[2], v[3], v[4], v[5]))
            })
            .collect()
    };
    let red = lines("1 0 0");
    let horizontal = red.iter().filter(|(_, y1, _, y2)| (y1 - y2).abs() < 0.01).count();
    assert_eq!(horizontal, 2, "one top and one bottom for the two paragraphs: {red:?}");
    assert!(red.iter().any(|(x1, y1, x2, y2)| (x1 - x2).abs() < 0.01 && (y1 - y2).abs() > 1.0), "side borders");
    let blue = lines("0 0 1");
    assert_eq!(blue.len(), 2, "a double border is two lines: {blue:?}");
}

#[test]
fn table_styles_shade_header_and_banded_rows_and_cells_align_vertically() {
    let cell = |t: &str, extra: &str| {
        format!(r#"<w:tc><w:tcPr>{extra}</w:tcPr><w:p><w:pPr><w:spacing w:after="0"/></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p></w:tc>"#)
    };
    let body = format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Banded"/><w:tblLook w:firstRow="1" w:noHBand="0"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>
        <w:tr>{}{}</w:tr><w:tr>{}{}</w:tr><w:tr><w:trPr><w:trHeight w:val="1440" w:hRule="exact"/></w:trPr>{}{}</w:tr></w:tbl>"#,
        cell("Cabeca", ""),
        cell("H2", ""),
        cell("Faixa", ""),
        cell("F2", ""),
        cell("Topo", ""),
        cell("Baixo", r#"<w:vAlign w:val="bottom"/>"#)
    );
    let (doc, _) = from_office("t.docx", &docx(&body, &[], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("0.267 0.447 0.769 rg"), "the header row's shading: {text}");
    assert!(text.contains("0.851 0.886 0.953 rg"), "the first band's shading");
    assert_eq!(colour_of(&text, "Cabeca"), "1 1 1", "the header row's white text");
    assert!(text.contains("/F1 11 Tf (Cabeca)"), "and bold (Helvetica-Bold): {text}");
    assert_eq!(colour_of(&text, "Faixa"), "0 0 0");
    let drop = y_of(&text, "Topo") - y_of(&text, "Baixo");
    assert!(drop > 50.0, "bottom-aligned text sits at the foot of the 72 pt row: {drop}");
}

#[test]
fn header_tables_are_laid_out_as_tables() {
    let header = format!(
        r#"<w:hdr xmlns:w="{W}"><w:tbl><w:tblGrid><w:gridCol w:w="4680"/><w:gridCol w:w="4680"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>Esquerda</w:t></w:r></w:p></w:tc><w:tc><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t>Direita</w:t></w:r></w:p><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t>Segunda</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:hdr>"#
    );
    let body = format!(r#"{}<w:sectPr><w:headerReference w:type="default" r:id="rIdH"/></w:sectPr>"#, p("Corpo"));
    let (doc, _) = from_office("h.docx", &docx(&body, &[("word/header1.xml", header.as_bytes())], &[("rIdH", "header", "header1.xml")])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert_eq!(y_of(&text, "Esquerda"), y_of(&text, "Direita"), "the two cells side by side");
    assert!(x_of(&text, "Direita") > 450.0, "right-aligned in the right-hand cell: {}", x_of(&text, "Direita"));
    assert!(y_of(&text, "Segunda") < y_of(&text, "Direita"), "the cell's second paragraph below its first");
}

#[test]
fn space_before_is_kept_on_the_first_page_and_bottom_floating_tables_sit_at_the_margin() {
    let body = format!(
        r#"<w:p><w:pPr><w:spacing w:before="2400"/></w:pPr><w:r><w:t>Titulo</w:t></w:r></w:p>
        <w:tbl><w:tblPr><w:tblpPr w:horzAnchor="margin" w:tblpYSpec="bottom"/></w:tblPr><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>Rodape</w:t></w:r></w:p></w:tc></w:tr></w:tbl>{}"#,
        p("Depois")
    );
    let (doc, _) = from_office("f.docx", &docx(&body, &[], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(y_of(&text, "Titulo") < 792.0 - 72.0 - 120.0, "120 pt before the first paragraph: {}", y_of(&text, "Titulo"));
    assert!(y_of(&text, "Rodape") < 72.0 + 30.0, "the table ends at the bottom margin: {}", y_of(&text, "Rodape"));
    assert!(y_of(&text, "Depois") < y_of(&text, "Titulo") && y_of(&text, "Depois") > 500.0, "the text goes on where it was");
}

#[test]
fn emf_pictures_are_drawn_as_vector_forms() {
    let emf = super::emf::tests::emf(&super::emf::tests::logo_records());
    let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="635000"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rIdE"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
    let (doc, report) = from_office("e.docx", &docx(body, &[("word/media/image1.emf", &emf)], &[("rIdE", "image", "media/image1.emf")])).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("empty boxes")), "{report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    let xo = pg[0].get(b"Resources").unwrap().as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap().clone();
    let form = doc.resolve(xo.get(b"Im0").unwrap());
    let Object::Stream(s) = &*form else { panic!("a stream") };
    assert_eq!(s.dict.name(b"Subtype"), Some(&b"Form"[..]));
    assert!(String::from_utf8_lossy(&s.decoded().unwrap()).contains("(Logo) Tj"));
}

/// A worksheet's relationships part.
fn sheet_rels(items: &[(&str, &str, &str)]) -> String {
    rels(items)
}

#[test]
fn excel_headers_and_footers_print_fields_and_pictures() {
    let image = png(8, 4);
    let vml = r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office"><v:shape id="LH" style="position:absolute;width:40pt;height:20pt"><v:imagedata o:relid="rId1" o:title="logo"/></v:shape></xml>"#;
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Um</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Dois</t></is></c></row></sheetData>
      <headerFooter><oddHeader>&amp;L&amp;G&amp;C&amp;"Arial,Bold"Relatorio&amp;R&amp;A</oddHeader><oddFooter>&amp;CPagina &amp;P de &amp;N</oddFooter></headerFooter>
      <rowBreaks count="1" manualBreakCount="1"><brk id="1" max="16383" man="1"/></rowBreaks><legacyDrawingHF r:id="rIdV"/>"#;
    let rels_xml = sheet_rels(&[("rIdV", "vmlDrawing", "../drawings/vmlDrawing1.vml")]);
    let vml_rels = sheet_rels(&[("rId1", "image", "../media/image1.png")]);
    let extra: [(&str, &[u8]); 4] = [
        ("xl/worksheets/_rels/sheet1.xml.rels", rels_xml.as_bytes()),
        ("xl/drawings/vmlDrawing1.vml", vml.as_bytes()),
        ("xl/drawings/_rels/vmlDrawing1.vml.rels", vml_rels.as_bytes()),
        ("xl/media/image1.png", &image),
    ];
    let (doc, report) = from_office("hf.xlsx", &xlsx_full(sheet, &[], "", "0", None, &extra)).unwrap();
    assert_eq!(report.pages, 2, "the manual break starts a page: {report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    let first = content(&doc, &pg[0]);
    let second = content(&doc, &pg[1]);
    assert!(first.contains("(Relatorio)") && first.contains("(Vendas)"), "title and sheet name: {first}");
    assert!(first.contains("(Pagina 1 de 2)") && second.contains("(Pagina 2 de 2)"), "page fields: {second}");
    assert!(first.contains("/Im0 Do"), "the header picture");
    assert!(second.contains("(Dois)") && !second.contains("(Um)"));
}

#[test]
fn excel_print_titles_repeat_and_print_areas_print_alone_centred() {
    let rows: String =
        (1..=80).map(|r| format!(r#"<row r="{r}" ht="15" customHeight="1"><c r="A{r}" t="inlineStr"><is><t>Linha{r}</t></is></c></row>"#)).collect();
    let titles = r#"<definedNames><definedName name="_xlnm.Print_Titles" localSheetId="0">Vendas!$1:$1</definedName></definedNames>"#;
    let (doc, report) = from_office("t.xlsx", &xlsx_with(&format!("<sheetData>{rows}</sheetData>"), &[], titles)).unwrap();
    assert!(report.pages >= 2, "{report:?}");
    let doc = reopen(&doc);
    let second = content(&doc, &pages(&doc)[1]);
    assert!(second.contains("(Linha1)") && second.contains("(Linha80)"), "the title row on the second page too: {second}");
    assert!(y_of(&second, "Linha1") > y_of(&second, "Linha80"), "at its top");

    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Dentro</t></is></c><c r="B1" t="inlineStr"><is><t>Fora</t></is></c></row><row r="5"><c r="C5" t="inlineStr"><is><t>Segunda</t></is></c></row></sheetData><printOptions horizontalCentered="1"/>"#;
    let areas =
        r#"<definedNames><definedName name="_xlnm.Print_Area" localSheetId="0">'Vendas'!$A$1:$A$1,'Vendas'!$C$5</definedName></definedNames>"#;
    let (doc, report) = from_office("a.xlsx", &xlsx_with(sheet, &[], areas)).unwrap();
    assert_eq!(report.pages, 2, "each print area on its own page: {report:?}");
    let doc = reopen(&doc);
    let text = all_text(&doc);
    assert!(text.contains("(Dentro)") && text.contains("(Segunda)") && !text.contains("(Fora)"), "{text}");
    let first = content(&doc, &pages(&doc)[0]);
    assert!(first.contains(" cm\n"), "moved to the middle of the page: {first}");
}

const WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";

#[test]
fn word_shapes_groups_and_turned_floating_boxes_are_drawn_with_their_text() {
    let wsp = |xfrm: &str, geom: &str, fill: &str, text: &str| {
        let body = if text.is_empty() {
            String::new()
        } else {
            format!(r#"<wps:txbx><w:txbxContent><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:txbxContent></wps:txbx>"#)
        };
        format!(
            r#"<wps:wsp xmlns:wps="{WPS}"><wps:spPr>{xfrm}<a:prstGeom prst="{geom}"/><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill></wps:spPr>{body}<wps:bodyPr/></wps:wsp>"#
        )
    };
    let at = |x: u32, rot: &str| format!(r#"<a:xfrm{rot}><a:off x="{x}" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm>"#);
    let group = format!(
        r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="2540000" cy="1270000"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"><wpg:wgp xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"><wpg:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2540000" cy="1270000"/><a:chOff x="0" y="0"/><a:chExt cx="2540000" cy="1270000"/></a:xfrm></wpg:grpSpPr>{}{}</wpg:wgp></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
        wsp(&at(0, ""), "ellipse", "FF0000", "Elipse"),
        wsp(&at(1_270_000, ""), "rect", "00FF00", ""),
    );
    let turned = format!(
        r#"<w:p><w:r><w:drawing><wp:anchor behindDoc="0" distT="0" distB="0" distL="0" distR="0"><wp:positionH relativeFrom="column"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="1270000" cy="1270000"/><wp:wrapTopAndBottom/><a:graphic><a:graphicData uri="{WPS}">{}</a:graphicData></a:graphic></wp:anchor></w:drawing></w:r><w:r><w:t>Depois</w:t></w:r></w:p>"#,
        wsp(&at(0, r#" rot="5400000""#), "rect", "0000FF", "Girada"),
    );
    let (doc, report) = from_office("shapes.docx", &docx(&format!("{group}{turned}"), &[], &[])).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("text boxes") || n.contains("shapes")), "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    for needle in ["(Elipse)", "(Girada)", "(Depois)", "1 0 0 rg", "0 1 0 rg", "0 0 1 rg"] {
        assert!(text.contains(needle), "missing {needle}: {text}");
    }
    assert!(text.contains("0 -1 1 0 "), "the floating box turned a quarter clockwise: {text}");
    assert!(y_of(&text, "Depois") < y_of(&text, "Elipse") - 90.0, "the text goes on below the top-and-bottom box");
}

/// A SmartArt diagram's data part pointing at its saved drawing, and that drawing: one rounded
/// box with text.
fn smartart_parts() -> (String, String) {
    let data = format!(
        r#"<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="{A}"><dgm:ptLst/><dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" relId="rIdDr" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></dgm:extLst></dgm:dataModel>"#
    );
    let drawing = format!(
        r#"<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="{A}"><dsp:spTree><dsp:nvGrpSpPr/><dsp:grpSpPr/><dsp:sp><dsp:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="roundRect"/><a:solidFill><a:srgbClr val="FFA500"/></a:solidFill></dsp:spPr><dsp:txBody><a:bodyPr/><a:p><a:r><a:t>Etapa</a:t></a:r></a:p></dsp:txBody></dsp:sp></dsp:spTree></dsp:drawing>"#
    );
    (data, drawing)
}

const REL_IDS: &str =
    r#"<dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rIdDm" r:lo="rIdLo" r:qs="rIdQs" r:cs="rIdCs"/>"#;

#[test]
fn smartart_is_drawn_from_its_saved_drawing_in_word_and_excel() {
    let (data, drawing) = smartart_parts();
    let frame = format!(
        r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="2540000" cy="1270000"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">{REL_IDS}</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
    );
    let (doc, report) = from_office(
        "sa.docx",
        &docx(
            &frame,
            &[("word/diagrams/data1.xml", data.as_bytes()), ("word/diagrams/drawing1.xml", drawing.as_bytes())],
            &[("rIdDm", "diagramData", "diagrams/data1.xml"), ("rIdDr", "diagramDrawing", "diagrams/drawing1.xml")],
        ),
    )
    .unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("SmartArt")), "{report:?}");
    let text = content(&reopen(&doc), &pages(&reopen(&doc))[0]);
    assert!(text.contains("(Etapa)") && text.contains("1 0.647 0 rg"), "the box and its text: {text}");

    const XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    let sheet_drawing = format!(
        r#"<xdr:wsDr xmlns:xdr="{XDR}" xmlns:a="{A}" xmlns:r="{R}"><xdr:oneCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="2540000" cy="1270000"/><xdr:graphicFrame><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Diagram 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">{REL_IDS}</a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:oneCellAnchor></xdr:wsDr>"#
    );
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Dados</t></is></c></row></sheetData><drawing r:id="rIdD"/>"#;
    let rels_xml = sheet_rels(&[("rIdD", "drawing", "../drawings/drawing1.xml")]);
    let drawing_rels = sheet_rels(&[("rIdDm", "diagramData", "../diagrams/data1.xml"), ("rIdDr", "diagramDrawing", "../diagrams/drawing1.xml")]);
    let extra: [(&str, &[u8]); 5] = [
        ("xl/worksheets/_rels/sheet1.xml.rels", rels_xml.as_bytes()),
        ("xl/drawings/drawing1.xml", sheet_drawing.as_bytes()),
        ("xl/drawings/_rels/drawing1.xml.rels", drawing_rels.as_bytes()),
        ("xl/diagrams/data1.xml", data.as_bytes()),
        ("xl/diagrams/drawing1.xml", drawing.as_bytes()),
    ];
    let (doc, report) = from_office("sa.xlsx", &xlsx_full(sheet, &[], "", "0", None, &extra)).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("SmartArt") || n.contains("drawing")), "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(Etapa)") && text.contains("1 0.647 0 rg"), "the sheet's diagram: {text}");
}

#[test]
fn word_equations_are_laid_out_as_mathematics() {
    let body = r#"<w:p><m:oMathPara xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:oMath><m:f><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f><m:r><m:t>=</m:t></m:r><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath></m:oMathPara></w:p>"#;
    let (doc, report) = from_office("eq.docx", &docx(body, &[], &[])).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("equation")), "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(y_of(&text, "a") > y_of(&text, "b") + 4.0, "the numerator over the denominator: {text}");
    assert!(y_of(&text, "2") > y_of(&text, "x"), "the exponent raised");
    assert!(x_of(&text, "a") > 200.0, "a display equation is centred: {text}");
}

#[test]
fn formula_rules_format_the_cells_their_formula_holds_for() {
    let sheet = r#"<sheetData><row r="1"><c r="A1"><v>3</v></c><c r="B1"><v>1</v></c></row><row r="2"><c r="A2"><v>1</v></c><c r="B2"><v>9</v></c></row></sheetData>
      <conditionalFormatting sqref="A1:B2"><cfRule type="expression" dxfId="0" priority="1"><formula>AND($A1&gt;2,A1&lt;5)</formula></cfRule></conditionalFormatting>"#;
    let (doc, report) = from_office("cf2.xlsx", &xlsx(sheet, &[])).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("formula")), "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    // Row 1: $A1 is 3, and both cells are under 5; row 2's $A2 is 1.
    assert_eq!(text.matches("1 0.78 0.808 rg").count(), 2, "A1 and B1 filled: {text}");
    assert!(text.contains("/F1 11 Tf (3)") && text.contains("/F1 11 Tf (1)") && text.contains("/F0 11 Tf (9)"), "{text}");
}

#[test]
fn wmf_pictures_in_word_are_drawn_as_vectors() {
    use super::wmf::tests::{rec, wmf};
    let meta = wmf(&[rec(0x02FC, &[0, 0x00FF, 0, 0]), rec(0x012D, &[0]), rec(0x041B, &[700, 1400, 20, 20])]);
    let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="457200"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rIdImg"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
    let (doc, report) =
        from_office("wmf.docx", &docx(body, &[("word/media/image1.wmf", &meta)], &[("rIdImg", "image", "media/image1.wmf")])).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("can't draw")), "{report:?}");
    let doc = reopen(&doc);
    assert!(content(&doc, &pages(&doc)[0]).contains("/Im0 Do"), "the metafile's form");
}

#[test]
fn conditional_formatting_fills_matching_cells() {
    // The range ends on the sheet's last row: walking past it once panicked.
    let sheet = r#"<sheetData><row r="1"><c r="A1"><v>3</v></c><c r="B1"><v>1</v></c></row><row r="2"><c r="A2"><v>7</v></c><c r="B2"><v>9</v></c></row></sheetData>
      <conditionalFormatting sqref="A1:A2"><cfRule type="cellIs" dxfId="0" priority="1" operator="greaterThan"><formula>5</formula></cfRule></conditionalFormatting>
      <conditionalFormatting sqref="B1:B2"><cfRule type="colorScale" priority="2"><colorScale><cfvo type="min"/><cfvo type="max"/><color rgb="FFFFFFFF"/><color rgb="FF00FF00"/></colorScale></cfRule></conditionalFormatting>
      <conditionalFormatting sqref="C1:C2"><cfRule type="expression" dxfId="0" priority="3"><formula>MOD(C1,2)=0</formula></cfRule></conditionalFormatting>"#;
    let (doc, report) = from_office("cf.xlsx", &xlsx(sheet, &[])).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("formula")), "the MOD rule is evaluated: {report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert_eq!(text.matches("1 0.78 0.808 rg").count(), 1, "only the cell over 5 is filled: {text}");
    assert!(text.contains("/F1 11 Tf (7)"), "and bold");
    assert!(text.contains("0 1 0 rg") && text.contains("1 1 1 rg"), "the colour scale's ends");
}

#[test]
fn excel_sheet_pictures_and_shapes_are_drawn_where_they_are_anchored() {
    let image = png(10, 10);
    const XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    let drawing = format!(
        r#"<xdr:wsDr xmlns:xdr="{XDR}" xmlns:a="{A}" xmlns:r="{R}">
        <xdr:oneCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="635000" cy="635000"/><xdr:pic><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill><xdr:spPr/></xdr:pic><xdr:clientData/></xdr:oneCellAnchor>
        <xdr:twoCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>30</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>34</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:sp><xdr:spPr><a:prstGeom prst="roundRect"/><a:solidFill><a:srgbClr val="4472C4"/></a:solidFill></xdr:spPr><xdr:txBody><a:bodyPr anchor="ctr"/><a:p><a:pPr algn="ctr"/><a:r><a:rPr sz="1000"/><a:t>Forma</a:t></a:r></a:p></xdr:txBody></xdr:sp><xdr:clientData/></xdr:twoCellAnchor>
        <xdr:oneCellAnchor><xdr:from><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="12700000" cy="12700000"/><xdr:graphicFrame><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"/></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:oneCellAnchor></xdr:wsDr>"#
    );
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Dados</t></is></c></row></sheetData><drawing r:id="rIdD"/>"#;
    let rels_xml = sheet_rels(&[("rIdD", "drawing", "../drawings/drawing1.xml")]);
    let drawing_rels = sheet_rels(&[("rId1", "image", "../media/image1.png")]);
    let extra: [(&str, &[u8]); 4] = [
        ("xl/worksheets/_rels/sheet1.xml.rels", rels_xml.as_bytes()),
        ("xl/drawings/drawing1.xml", drawing.as_bytes()),
        ("xl/drawings/_rels/drawing1.xml.rels", drawing_rels.as_bytes()),
        ("xl/media/image1.png", &image),
    ];
    let (doc, report) = from_office("d.xlsx", &xlsx_full(sheet, &[], "", "0", None, &extra)).unwrap();
    assert!(report.notes.iter().any(|n| n.contains("charts on sheets")), "{report:?}");
    let doc = reopen(&doc);
    let text = all_text(&doc);
    assert!(text.contains("/Im0 Do") && text.contains("(Forma)"), "the picture and the shape's text: {text}");
    assert!(text.contains("0.267 0.447 0.769 rg"), "the shape's fill");
}

// ── fonts, columns, charts, footnotes, floats, Excel headings ───────────────────────────────

const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

/// A clustered column chart (or a pie with percentages) with literal values.
fn chart_xml(pie: bool) -> String {
    let series = r#"<c:ser><c:tx><c:v>Norte</c:v></c:tx><c:spPr><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></c:spPr><c:cat><c:strLit><c:pt idx="0"><c:v>T1</c:v></c:pt><c:pt idx="1"><c:v>T2</c:v></c:pt></c:strLit></c:cat><c:val><c:numLit><c:pt idx="0"><c:v>3</c:v></c:pt><c:pt idx="1"><c:v>5</c:v></c:pt></c:numLit></c:val></c:ser>"#;
    let group = if pie {
        format!(r#"<c:pieChart><c:varyColors val="1"/>{series}<c:dLbls><c:showPercent val="1"/></c:dLbls></c:pieChart>"#)
    } else {
        format!(
            r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/>{series}</c:barChart><c:catAx><c:axId val="1"/></c:catAx><c:valAx><c:axId val="2"/><c:majorGridlines/></c:valAx>"#
        )
    };
    format!(
        r#"<c:chartSpace xmlns:c="{C}" xmlns:a="{A}"><c:chart><c:title><c:tx><c:rich><a:p><a:r><a:t>Vendas</a:t></a:r></a:p></c:rich></c:tx></c:title><c:plotArea>{group}</c:plotArea><c:legend><c:legendPos val="b"/></c:legend></c:chart></c:chartSpace>"#
    )
}

#[test]
fn word_charts_are_drawn_from_their_cached_values() {
    let frame = |id: &str| {
        format!(
            r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="3810000" cy="2540000"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="{C}" r:id="{id}"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
        )
    };
    let body = format!("{}{}", frame("rIdC1"), frame("rIdC2"));
    let (bar, pie) = (chart_xml(false), chart_xml(true));
    let (doc, report) = from_office(
        "g.docx",
        &docx(
            &body,
            &[("word/charts/chart1.xml", bar.as_bytes()), ("word/charts/chart2.xml", pie.as_bytes())],
            &[("rIdC1", "chart", "charts/chart1.xml"), ("rIdC2", "chart", "charts/chart2.xml")],
        ),
    )
    .unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("charts")), "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    for needle in ["(Vendas)", "(Norte)", "(T1)", "(T2)", "(38%)", "(63%)"] {
        assert!(text.contains(needle), "missing {needle}: {text}");
    }
    assert!(text.contains("1 0 0 rg"), "the series' red bars");
}

#[test]
fn radar_and_surface_charts_draw_their_categories_and_values() {
    let ser = |name: &str, colour: &str, vals: [u32; 3]| {
        let pts: String = vals.iter().enumerate().map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#)).collect();
        format!(
            r#"<c:ser><c:tx><c:v>{name}</c:v></c:tx><c:spPr><a:solidFill><a:srgbClr val="{colour}"/></a:solidFill><a:ln><a:solidFill><a:srgbClr val="{colour}"/></a:solidFill></a:ln></c:spPr><c:cat><c:strLit><c:pt idx="0"><c:v>Ka</c:v></c:pt><c:pt idx="1"><c:v>Kb</c:v></c:pt><c:pt idx="2"><c:v>Kc</c:v></c:pt></c:strLit></c:cat><c:val><c:numLit>{pts}</c:numLit></c:val></c:ser>"#
        )
    };
    let chart =
        |group: String| format!(r#"<c:chartSpace xmlns:c="{C}" xmlns:a="{A}"><c:chart><c:plotArea>{group}</c:plotArea></c:chart></c:chartSpace>"#);
    let radar = chart(format!(r#"<c:radarChart><c:radarStyle val="marker"/>{}</c:radarChart>"#, ser("Rs", "00FF00", [2, 4, 3])));
    let surface = chart(format!(r#"<c:surfaceChart>{}{}</c:surfaceChart>"#, ser("Sa", "0000FF", [1, 5, 9]), ser("Sb", "0000FF", [2, 6, 8])));
    let frame = |id: &str| {
        format!(
            r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="3810000" cy="2540000"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="{C}" r:id="{id}"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
        )
    };
    let body = format!("{}{}", frame("rIdC1"), frame("rIdC2"));
    let (doc, report) = from_office(
        "rs.docx",
        &docx(
            &body,
            &[("word/charts/chart1.xml", radar.as_bytes()), ("word/charts/chart2.xml", surface.as_bytes())],
            &[("rIdC1", "chart", "charts/chart1.xml"), ("rIdC2", "chart", "charts/chart2.xml")],
        ),
    )
    .unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("chart")), "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    for needle in ["(Ka)", "(Kb)", "(Kc)", "(Sa)", "(Sb)"] {
        assert!(text.contains(needle), "missing {needle}: {text}");
    }
    assert!(text.contains("0 1 0 RG"), "the radar series' green outline: {text}");
    assert!(text.matches(" re f").count() >= 6, "one coloured cell per surface value: {text}");
}

#[test]
fn sheet_groups_sparklines_and_charts_with_two_axes_trendlines_and_labels() {
    const XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    let sp = |x: u32, prst: &str, fill: &str| {
        format!(
            r#"<xdr:sp><xdr:nvSpPr><xdr:cNvPr id="3" name="s"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="{x}" y="0"/><a:ext cx="500000" cy="500000"/></a:xfrm><a:prstGeom prst="{prst}"/><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill></xdr:spPr></xdr:sp>"#
        )
    };
    let drawing = format!(
        r#"<xdr:wsDr xmlns:xdr="{XDR}" xmlns:a="{A}" xmlns:r="{R}"><xdr:oneCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>3</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="1000000" cy="500000"/><xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="2" name="g"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1000000" cy="500000"/><a:chOff x="0" y="0"/><a:chExt cx="1000000" cy="500000"/></a:xfrm></xdr:grpSpPr>{}{}</xdr:grpSp><xdr:clientData/></xdr:oneCellAnchor><xdr:oneCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>8</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="3810000" cy="2540000"/><xdr:graphicFrame><a:graphic><a:graphicData uri="{C}"><c:chart xmlns:c="{C}" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:oneCellAnchor></xdr:wsDr>"#,
        sp(0, "rect", "FF0000"),
        sp(500_000, "ellipse", "0000FF"),
    );
    let ser = |name: &str, vals: [u32; 3], extra: &str| {
        let pts: String = vals.iter().enumerate().map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#)).collect();
        format!(r#"<c:ser><c:tx><c:v>{name}</c:v></c:tx>{extra}<c:val><c:numLit>{pts}</c:numLit></c:val></c:ser>"#)
    };
    let chart = format!(
        r#"<c:chartSpace xmlns:c="{C}" xmlns:a="{A}"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/>{}<c:axId val="1"/><c:axId val="2"/></c:barChart><c:lineChart><c:grouping val="standard"/>{}<c:axId val="3"/><c:axId val="4"/></c:lineChart><c:catAx><c:axId val="1"/><c:axPos val="b"/></c:catAx><c:valAx><c:axId val="2"/><c:axPos val="l"/></c:valAx><c:catAx><c:axId val="3"/><c:axPos val="b"/><c:delete val="1"/></c:catAx><c:valAx><c:axId val="4"/><c:axPos val="r"/></c:valAx></c:plotArea></c:chart></c:chartSpace>"#,
        ser("Barras", [3, 5, 4], r#"<c:trendline><c:trendlineType val="linear"/></c:trendline>"#),
        ser("Linha", [100, 300, 200], r#"<c:dLbls><c:showVal val="1"/></c:dLbls>"#),
    );
    let sheet = r#"<sheetData><row r="1"><c r="A1"><v>1</v></c><c r="B1"><v>3</v></c><c r="C1"><v>2</v></c></row></sheetData><drawing r:id="rIdD"/>
      <extLst><ext uri="{05C60535-1F16-4fd2-B633-F4F36F0B64E0}" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:sparklineGroups xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><x14:sparklineGroup type="column"><x14:colorSeries rgb="FF00AA00"/><x14:sparklines><x14:sparkline><xm:f>Sheet1!A1:C1</xm:f><xm:sqref>E1</xm:sqref></x14:sparkline></x14:sparklines></x14:sparklineGroup></x14:sparklineGroups></ext></extLst>"#;
    let rels_xml = sheet_rels(&[("rIdD", "drawing", "../drawings/drawing1.xml")]);
    let drawing_rels = sheet_rels(&[("rId1", "chart", "../charts/chart1.xml")]);
    let extra: [(&str, &[u8]); 4] = [
        ("xl/worksheets/_rels/sheet1.xml.rels", rels_xml.as_bytes()),
        ("xl/drawings/drawing1.xml", drawing.as_bytes()),
        ("xl/drawings/_rels/drawing1.xml.rels", drawing_rels.as_bytes()),
        ("xl/charts/chart1.xml", chart.as_bytes()),
    ];
    let (doc, report) = from_office("g.xlsx", &xlsx_full(sheet, &[], "", "0", None, &extra)).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("grouped")), "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("1 0 0 rg") && text.contains("0 0 1 rg"), "the group's two shapes: {text}");
    assert_eq!(text.matches("0 0.667 0 rg").count(), 3, "a sparkline column per value");
    assert!(x_of(&text, "250") > x_of(&text, "5") + 100.0, "the line's own axis on the right");
    assert!(text.contains("(100)") && text.contains("(300)"), "the line's values written at its points");
    assert!(text.contains("[1.5 2] 0 d"), "the bars' dotted trendline");
}

#[test]
fn excel_charts_on_sheets_and_chart_sheets_are_drawn() {
    const XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    let drawing = format!(
        r#"<xdr:wsDr xmlns:xdr="{XDR}" xmlns:a="{A}" xmlns:r="{R}"><xdr:oneCellAnchor><xdr:from><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="3810000" cy="2540000"/><xdr:graphicFrame><a:graphic><a:graphicData uri="{C}"><c:chart xmlns:c="{C}" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:oneCellAnchor></xdr:wsDr>"#
    );
    let chart = chart_xml(false);
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Dados</t></is></c></row></sheetData><drawing r:id="rIdD"/>"#;
    let chart_sheet = r#"<pageMargins left="0.5" right="0.5" top="0.5" bottom="0.5" header="0.3" footer="0.3"/><drawing r:id="rIdD"/>"#;
    let rels_xml = sheet_rels(&[("rIdD", "drawing", "../drawings/drawing1.xml")]);
    let drawing_rels = sheet_rels(&[("rId1", "chart", "../charts/chart1.xml")]);
    let extra: [(&str, &[u8]); 5] = [
        ("xl/worksheets/_rels/sheet1.xml.rels", rels_xml.as_bytes()),
        ("xl/worksheets/_rels/sheet2.xml.rels", rels_xml.as_bytes()),
        ("xl/drawings/drawing1.xml", drawing.as_bytes()),
        ("xl/drawings/_rels/drawing1.xml.rels", drawing_rels.as_bytes()),
        ("xl/charts/chart1.xml", chart.as_bytes()),
    ];
    let (doc, report) = from_office("c.xlsx", &xlsx_full(sheet, &[("Grafico1", chart_sheet, false)], "", "0", None, &extra)).unwrap();
    assert_eq!(report.pages, 2, "the sheet and the chart sheet: {report:?}");
    assert!(!report.notes.iter().any(|n| n.contains("chart")), "{report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert!(content(&doc, &pg[0]).contains("(Vendas)"), "the chart on the sheet");
    assert!(content(&doc, &pg[1]).contains("(Vendas)"), "the chart sheet's chart");
    let (w, h) = size(&pg[1]);
    assert!(w > h, "a chart sheet prints landscape: {w}x{h}");
}

#[test]
fn sections_with_columns_flow_break_balance_and_draw_their_lines() {
    let para = |t: &str| format!(r#"<w:p><w:pPr><w:spacing w:after="0"/></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#);
    let cols = r#"<w:sectPr><w:type w:val="continuous"/><w:cols w:num="2" w:space="720" w:sep="1"/></w:sectPr>"#;
    let one = r#"<w:sectPr><w:type w:val="continuous"/></w:sectPr>"#;
    let body = format!(
        r#"{}<w:p><w:pPr>{one}</w:pPr></w:p>{}{}<w:p><w:r><w:t>Antes</w:t><w:br w:type="column"/><w:t>Depois</w:t></w:r></w:p>{}{}<w:p><w:pPr>{cols}</w:pPr></w:p>{}{one}"#,
        para("Titulo"),
        para("Linha1"),
        para("Linha2"),
        para("Linha3"),
        para("Linha4"),
        para("Fim")
    );
    let (doc, _) = from_office("cols.docx", &docx(&body, &[], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    let mid = 72.0 + 468.0 / 2.0;
    assert!(x_of(&text, "Linha1") < mid && x_of(&text, "Antes") < mid, "the first column");
    assert!(x_of(&text, "Depois") > mid, "after the column break, the second column");
    assert_eq!(y_of(&text, "Depois"), y_of(&text, "Linha1"), "the second column starts at the columns' top");
    let lowest = y_of(&text, "Linha4").min(y_of(&text, "Antes"));
    assert!(y_of(&text, "Fim") < lowest, "one column again below the lowest column");
    assert!(
        text.lines().any(|l| l.starts_with("0 0 0 RG 0.5 w 0 J") && {
            let v: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            v.len() >= 7 && (v[5] - mid).abs() < 1.0 && (v[5] - v[7]).abs() < 0.01
        }),
        "a line between the columns: {text}"
    );

    // Columns ending at a continuous break are balanced: six short paragraphs, three a column.
    let many: String = (1..=6).map(|i| para(&format!("Item{i}"))).collect();
    let body = format!(r#"{many}<w:p><w:pPr>{cols}</w:pPr></w:p>{}<w:sectPr><w:type w:val="continuous"/></w:sectPr>"#, para("Final"));
    let (doc, _) = from_office("bal.docx", &docx(&body, &[], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(x_of(&text, "Item3") < mid && x_of(&text, "Item4") > mid, "{text}");
    assert_eq!(y_of(&text, "Item4"), y_of(&text, "Item1"));
}

#[test]
fn footnotes_print_at_the_foot_of_their_page() {
    let notes =
        format!(r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1"><w:p><w:r><w:t>Nota de rodape</w:t></w:r></w:p></w:footnote></w:footnotes>"#);
    let filler: String = (0..60).map(|i| p(&format!("Corpo {i}"))).collect();
    let body = format!(r#"<w:p><w:r><w:t>Texto</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p>{filler}"#);
    let (doc, report) =
        from_office("fn.docx", &docx(&body, &[("word/footnotes.xml", notes.as_bytes())], &[("rIdF", "footnotes", "footnotes.xml")])).unwrap();
    assert!(report.pages >= 2 && !report.notes.iter().any(|n| n.contains("footnote")), "{report:?}");
    let doc = reopen(&doc);
    let first = content(&doc, &pages(&doc)[0]);
    let y = y_of(&first, "rodape");
    assert!(y > 72.0 && y < 120.0, "at the foot of page 1, above the margin: {y}");
    let last_body = (0..60).rev().find(|i| first.contains(&format!("({i})"))).unwrap();
    assert!(y < y_of(&first, &last_body.to_string()), "below the page's text");
}

#[test]
fn a_footnote_goes_on_the_page_of_the_line_that_references_it() {
    let notes = format!(r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1"><w:p><w:r><w:t>Nota final</w:t></w:r></w:p></w:footnote></w:footnotes>"#);
    let long = "palavra ".repeat(150);
    let body = format!(
        r#"<w:p><w:r><w:t xml:space="preserve">{long}fim</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p><w:sectPr><w:pgSz w:w="6000" w:h="4000"/><w:pgMar w:top="720" w:right="720" w:bottom="720" w:left="720" w:header="360" w:footer="360"/></w:sectPr>"#
    );
    let (doc, report) =
        from_office("fn2.docx", &docx(&body, &[("word/footnotes.xml", notes.as_bytes())], &[("rIdF", "footnotes", "footnotes.xml")])).unwrap();
    assert!(report.pages >= 3, "{report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    let last = content(&doc, &pg[pg.len() - 1]);
    assert!(!content(&doc, &pg[0]).contains("(Nota)"), "not where the paragraph starts");
    assert!(last.contains("(Nota)") && last.contains("(fim)"), "with its reference's line: {last}");
}

#[test]
fn cropped_pictures_dashed_and_wavy_borders_and_shaded_cell_paragraphs() {
    let image = png(8, 4);
    let pic = r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="635000"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rIdImg"/><a:srcRect l="25000" r="25000"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#;
    let bordered = r#"<w:p><w:pPr><w:pBdr><w:top w:val="dotted" w:sz="8" w:space="1"/><w:bottom w:val="wave" w:sz="8" w:space="1"/></w:pBdr></w:pPr><w:r><w:t>Bordas</w:t></w:r></w:p>"#;
    let table = r#"<w:tbl><w:tblGrid><w:gridCol w:w="3000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcBorders><w:top w:val="dotDash" w:sz="4"/></w:tcBorders></w:tcPr><w:p><w:pPr><w:pBdr><w:left w:val="dashed" w:sz="4"/></w:pBdr><w:shd w:val="clear" w:fill="FFFF00"/></w:pPr><w:r><w:t>Celula</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#;
    let body = format!("{pic}{bordered}{table}");
    let (doc, _) = from_office("d.docx", &docx(&body, &[("word/media/image1.png", &image)], &[("rIdImg", "image", "media/image1.png")])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    // A quarter cut from each side: the image drawn twice as wide, a quarter of that to the
    // left of the 100 × 50 pt box it is clipped to.
    assert!(text.contains("re W n") && text.contains("q 200 0 0 50 22 "), "the crop: {text}");
    assert!(text.contains("[1 2] 0 d"), "the dotted top border: {text}");
    assert!(text.matches(" c ").count() >= 10, "the wavy bottom border's curves");
    assert!(text.contains("[3 1.5] 0 d"), "the cell paragraph's dashed left border");
    assert!(text.contains("[3 1 0.5 1] 0 d"), "the cell's dot-dash top border");
    assert!(text.contains("1 1 0 rg"), "the cell paragraph's yellow shading");
}

#[test]
fn floating_pictures_take_their_room_or_none_and_text_wraps_beside_square_ones() {
    let image = png(20, 20);
    let anchor = |wrap: &str, behind: u8| {
        format!(
            r#"<w:r><w:drawing><wp:anchor behindDoc="{behind}" distL="114300"><wp:positionH relativeFrom="margin"><wp:align>right</wp:align></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="2540000" cy="2540000"/>{wrap}<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rIdImg"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>"#
        )
    };
    let long = "palavra ".repeat(120);
    let convert = |wrap: &str, behind: u8| {
        let body = format!(r#"<w:p>{}<w:r><w:t xml:space="preserve">{long}Ultima</w:t></w:r></w:p>{}"#, anchor(wrap, behind), p("Seguinte"));
        let (doc, _) = from_office("f.docx", &docx(&body, &[("word/media/image1.png", &image)], &[("rIdImg", "image", "media/image1.png")])).unwrap();
        let doc = reopen(&doc);
        content(&doc, &pages(&doc)[0])
    };
    let none = convert("<wp:wrapNone/>", 1);
    let square = convert(r#"<wp:wrapSquare wrapText="bothSides"/>"#, 0);
    let top_bottom = convert("<wp:wrapTopAndBottom/>", 0);
    assert!(none.contains("/Im0 Do") && square.contains("/Im0 Do"));
    assert!(y_of(&none, "palavra") > 700.0, "a picture behind the text takes no room");
    assert!(y_of(&square, "Ultima") < y_of(&none, "Ultima") - 5.0, "lines beside a square picture are narrower, so the paragraph is longer");
    assert!(x_of(&square, "palavra") < 100.0, "the text is on the picture's left");
    assert!(y_of(&top_bottom, "palavra") < 720.0 - 190.0, "a top-and-bottom picture pushes the text below it");
}

#[test]
fn table_header_rows_repeat_on_each_page() {
    let row = |t: &str, header: bool| {
        let pr = if header { "<w:trPr><w:tblHeader/></w:trPr>" } else { "" };
        format!(r#"<w:tr>{pr}<w:tc><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:tc></w:tr>"#)
    };
    let rows: String = std::iter::once(row("Cabecalho", true)).chain((0..80).map(|i| row(&format!("Linha{i}"), false))).collect();
    let body = format!(r#"<w:tbl><w:tblGrid><w:gridCol w:w="5000"/></w:tblGrid>{rows}</w:tbl>"#);
    let (doc, report) = from_office("h.docx", &docx(&body, &[], &[])).unwrap();
    assert!(report.pages >= 2, "{report:?}");
    let doc = reopen(&doc);
    let second = content(&doc, &pages(&doc)[1]);
    assert!(second.contains("(Cabecalho)"), "{second}");
    assert!(y_of(&second, "Cabecalho") > 700.0, "at the top of the page");
}

#[test]
fn list_instances_of_one_abstract_list_share_their_count() {
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum>
        <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="0"/></w:num>
        <w:num w:numId="3"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride></w:num></w:numbering>"#
    );
    let item = |n: u8, t: &str| {
        format!(r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="{n}"/></w:numPr></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#)
    };
    let body = format!("{}{}{}{}", item(1, "Um"), item(1, "Dois"), item(2, "Tres"), item(3, "Recomeco"));
    let (doc, _) = from_office("n.docx", &docx_numbered(&body, &[], &[], Some(&numbering))).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert_eq!(y_of(&text, "3."), y_of(&text, "Tres"), "the other instance goes on counting: {text}");
    assert_eq!(text.matches("(1.)").count(), 2, "a start override restarts the count");
}

#[test]
fn excel_prints_row_and_column_headings_and_mac_column_widths() {
    let sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Um</t></is></c><c r="B1" t="inlineStr"><is><t>Dois</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Tres</t></is></c></row></sheetData><printOptions headings="1"/>"#;
    let (doc, _) = from_office("h.xlsx", &xlsx(sheet, &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    for needle in ["(A)", "(B)", "(1)", "(2)"] {
        assert!(text.contains(needle), "missing heading {needle}: {text}");
    }
    assert!(x_of(&text, "A") > x_of(&text, "1"), "column letters right of the row numbers");
    assert!(y_of(&text, "Um") < y_of(&text, "A"), "cells below the letters");

    // Column widths count in digits: Excel for Mac's are 8 pixels, so its columns are wider.
    let plain =
        r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Um</t></is></c><c r="B1" t="inlineStr"><is><t>Dois</t></is></c></row></sheetData>"#;
    let x_b = |extra: &[(&str, &[u8])]| {
        let (doc, _) = from_office("w.xlsx", &xlsx_full(plain, &[], "", "0", None, extra)).unwrap();
        let doc = reopen(&doc);
        x_of(&content(&doc, &pages(&doc)[0]), "Dois")
    };
    let app = b"<Properties><Application>Microsoft Macintosh Excel</Application></Properties>";
    assert!(x_b(&[("docProps/app.xml", app)]) > x_b(&[]) + 5.0);
}

#[test]
fn fonts_embedded_in_a_word_file_are_used_when_their_licence_allows() {
    // A document embedding a font under a name of its own, obfuscated as Word saves it; and a
    // damaged one. The font is craft-fonts' Carlito, so the first case needs a build with it.
    let key = "{6C5E6D9A-1B2C-4D3E-8F70-0A1B2C3D4E5F}";
    let obfuscate = |mut bytes: Vec<u8>| {
        let hex: String = key.chars().filter(char::is_ascii_hexdigit).collect();
        let mut k: Vec<u8> = (0..16).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
        k.reverse();
        for (j, b) in bytes.iter_mut().take(32).enumerate() {
            *b ^= k[j % 16];
        }
        bytes
    };
    let carlito = pdfcraft_fonts::CRAFT_FONTS.iter().find(|f| f.family == "Carlito" && f.style == "Regular").map(|f| f.bytes.to_vec());
    let convert = |font: Vec<u8>| {
        let table = format!(
            r#"<w:fonts xmlns:w="{W}" xmlns:r="{R}"><w:font w:name="Fonte Propria"><w:embedRegular r:id="rId1" w:fontKey="{key}"/></w:font></w:fonts>"#
        );
        let table_rels = rels(&[("rId1", "font", "fonts/font1.odttf")]);
        let body = r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="Fonte Propria" w:hAnsi="Fonte Propria"/></w:rPr><w:t>Letra propria</w:t></w:r></w:p>"#;
        let font = obfuscate(font);
        let bytes = docx(
            body,
            &[("word/fontTable.xml", table.as_bytes()), ("word/_rels/fontTable.xml.rels", table_rels.as_bytes()), ("word/fonts/font1.odttf", &font)],
            &[("rIdFT", "fontTable", "fontTable.xml")],
        );
        super::from_office("p.docx", &bytes).unwrap()
    };
    let (doc, report) = convert(b"not a font at all, only bytes long enough to be obfuscated".to_vec());
    assert!(report.notes.iter().any(|n| n.contains("fonts embedded in the document")), "a damaged font is refused: {report:?}");
    assert!(content(&reopen(&doc), &pages(&reopen(&doc))[0]).contains("(Letra"), "the text in a standard face");
    if let (Some(font), true) = (carlito, super::faces::embedding_on()) {
        let (doc, report) = convert(font);
        assert!(!report.notes.iter().any(|n| n.contains("fonts embedded")), "{report:?}");
        let doc = reopen(&doc);
        let pg = pages(&doc);
        let fonts = pg[0].get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
        assert!(
            fonts.iter().any(|(k, _)| k.starts_with(b"E100")),
            "the document's own font, in its slot: {:?}",
            fonts.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn office_faces_are_embedded_when_the_build_has_them() {
    let body = r#"<w:p><w:r><w:t>Calibri text</w:t></w:r></w:p><w:p><w:pPr><w:bidi/></w:pPr><w:r><w:t>שלום עולם</w:t></w:r></w:p>"#;
    let bytes = docx(body, &[], &[]);
    let (doc, report) = super::from_office("e.docx", &bytes).unwrap();
    let doc = reopen(&doc);
    let pg = pages(&doc);
    let fonts = pg[0].get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
    let names: Vec<String> =
        fonts.iter().map(|(_, r)| String::from_utf8_lossy(doc.resolve(r).as_dict().unwrap().name(b"BaseFont").unwrap()).into_owned()).collect();
    if super::faces::loaded(super::faces::Face::Carlito, false, false).is_some() {
        assert!(report.fonts_embedded >= 1, "{report:?}");
        assert!(names.iter().any(|n| n.ends_with("+Carlito-Regular")), "a subset of Carlito for Calibri: {names:?}");
        assert!(!report.notice().contains("uses standard PDF fonts"));
        if pdfcraft_fonts::document_hebrew_font(false).is_some() {
            assert_eq!(report.escaped, 0, "Hebrew is shaped in its face, not escaped: {report:?}");
        }
    } else {
        assert_eq!(report.fonts_embedded, 0);
        assert!(names.iter().any(|n| n == "Helvetica"), "{names:?}");
        assert!(report.notice().contains("uses standard PDF fonts"));
    }
    // Asking for the standard faces always gets them.
    let (_, std) = from_office_with("e.docx", &bytes, OfficeOptions { embed_fonts: false }).unwrap();
    assert_eq!(std.fonts_embedded, 0);
}
