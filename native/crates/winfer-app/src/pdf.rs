//! PDF pages as images, rendered on the CPU by `hayro` (replaces pdf.js in the viewer).

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::sync::Arc;

/// Pages bigger than this (pixels per side) are scaled down.
const MAX_SIDE: f32 = 4096.0;

pub struct Page {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn page_count(bytes: &Arc<Vec<u8>>) -> Result<usize, String> {
    let pdf = Pdf::new(bytes.clone()).map_err(|e| format!("{e:?}"))?;
    Ok(pdf.pages().len())
}

/// Page `index` (0-based), `scale` pixels per PDF point.
pub fn render_page(bytes: &Arc<Vec<u8>>, index: usize, scale: f32) -> Result<Page, String> {
    let pdf = Pdf::new(bytes.clone()).map_err(|e| format!("{e:?}"))?;
    let pages = pdf.pages();
    let page = pages.get(index).ok_or_else(|| format!("no page {}", index + 1))?;
    let (w, h) = page.render_dimensions();
    let scale = scale.min(MAX_SIDE / w.max(h).max(1.0)).max(0.05);
    let pixmap = render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE },
    );
    let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);
    let png = pixmap.into_png().map_err(|e| format!("{e:?}"))?;
    Ok(Page { png, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with a line of text, xref offsets computed.
    fn sample() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            {
                let stream = "BT /F1 24 Tf 20 40 Td (Hello) Tj ET";
                format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len())
            },
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }

    #[test]
    fn renders_a_page() {
        let bytes = Arc::new(sample());
        assert_eq!(page_count(&bytes).unwrap(), 1);
        let page = render_page(&bytes, 0, 2.0).unwrap();
        assert_eq!((page.width, page.height), (400, 200));
        assert!(page.png.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(render_page(&bytes, 1, 1.0).is_err());
        assert!(page_count(&Arc::new(b"not a pdf".to_vec())).is_err());
    }
}
