//! Parse Adobe/Resolve `.cube` 3D LUT files and sample them with tetrahedral
//! interpolation in display RGB space.

use editor_core::EmbeddedLut3D;
use std::fmt;
use std::path::Path;

/// Maximum LUT edge length embedded in project JSON (17³ = 4913 samples).
pub const LUT_EMBED_MAX_SIZE: u32 = 17;

#[derive(Clone, Debug, PartialEq)]
pub struct Lut3D {
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub title: Option<String>,
    /// Red slowest, green middle, blue fastest — standard `.cube` order.
    pub table: Vec<[f32; 3]>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LutError {
    Empty,
    MissingSize,
    InvalidSize(u32),
    TooFewSamples { expected: usize, found: usize },
    TooManySamples { expected: usize, found: usize },
    InvalidLine { line: usize, detail: String },
    Io(String),
}

impl fmt::Display for LutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "LUT file is empty"),
            Self::MissingSize => write!(f, "LUT file is missing LUT_3D_SIZE"),
            Self::InvalidSize(size) => write!(f, "LUT size must be at least 2, got {size}"),
            Self::TooFewSamples { expected, found } => {
                write!(f, "LUT expected {expected} RGB triples, found {found}")
            }
            Self::TooManySamples { expected, found } => {
                write!(f, "LUT expected {expected} RGB triples, found extra data at {found}")
            }
            Self::InvalidLine { line, detail } => {
                write!(f, "LUT parse error on line {line}: {detail}")
            }
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LutError {}

impl Lut3D {
    pub fn from_embedded(embedded: &EmbeddedLut3D) -> Result<Self, LutError> {
        let expected = embedded.size as usize * embedded.size as usize * embedded.size as usize;
        if embedded.size < 2 {
            return Err(LutError::InvalidSize(embedded.size));
        }
        if embedded.table.len() != expected {
            return Err(LutError::TooFewSamples {
                expected,
                found: embedded.table.len(),
            });
        }
        Ok(Self {
            size: embedded.size,
            domain_min: embedded.domain_min,
            domain_max: embedded.domain_max,
            title: None,
            table: embedded.table.clone(),
        })
    }

    pub fn to_embedded(&self) -> EmbeddedLut3D {
        EmbeddedLut3D {
            size: self.size,
            domain_min: self.domain_min,
            domain_max: self.domain_max,
            table: self.table.clone(),
        }
    }

    pub fn should_embed(&self) -> bool {
        self.size <= LUT_EMBED_MAX_SIZE
    }

    pub fn from_cube_str(text: &str) -> Result<Self, LutError> {
        if text.trim().is_empty() {
            return Err(LutError::Empty);
        }
        let mut title = None;
        let mut size = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut table = Vec::new();

        for (index, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let line_no = index + 1;
            if line.starts_with("TITLE") {
                title = Some(parse_quoted_or_tail(line, "TITLE")?);
                continue;
            }
            if line.starts_with("LUT_3D_SIZE") {
                size = Some(parse_size_line(line, line_no)?);
                continue;
            }
            if line.starts_with("DOMAIN_MIN") {
                domain_min = parse_triplet_line(line, "DOMAIN_MIN", line_no)?;
                continue;
            }
            if line.starts_with("DOMAIN_MAX") {
                domain_max = parse_triplet_line(line, "DOMAIN_MAX", line_no)?;
                continue;
            }
            if line.starts_with("LUT_1D") {
                return Err(LutError::InvalidLine {
                    line: line_no,
                    detail: "1D LUTs are not supported".into(),
                });
            }
            table.push(parse_rgb_line(line, line_no)?);
        }

        let size = size.ok_or(LutError::MissingSize)?;
        if size < 2 {
            return Err(LutError::InvalidSize(size));
        }
        let expected = size as usize * size as usize * size as usize;
        if table.len() < expected {
            return Err(LutError::TooFewSamples {
                expected,
                found: table.len(),
            });
        }
        if table.len() > expected {
            return Err(LutError::TooManySamples {
                expected,
                found: table.len(),
            });
        }
        Ok(Self {
            size,
            domain_min,
            domain_max,
            title,
            table,
        })
    }

    pub fn from_cube_file(path: &Path) -> Result<Self, LutError> {
        let text = std::fs::read_to_string(path).map_err(|err| LutError::Io(err.to_string()))?;
        let mut lut = Self::from_cube_str(&text)?;
        if lut.title.is_none() {
            lut.title = path
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
        }
        Ok(lut)
    }

    fn at(&self, r: u32, g: u32, b: u32) -> [f32; 3] {
        let index = (r * self.size * self.size + g * self.size + b) as usize;
        self.table[index]
    }

    fn map_input(&self, rgb: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|idx| {
            let min = self.domain_min[idx];
            let max = self.domain_max[idx];
            let span = (max - min).max(1.0e-6);
            ((rgb[idx] - min) / span).clamp(0.0, 1.0)
        })
    }

    /// Tetrahedral interpolation — industry default for 3D LUT sampling.
    pub fn sample_tetrahedral(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mapped = self.map_input(rgb);
        let size_f = self.size as f32;
        let scaled = mapped.map(|v| v * (size_f - 1.0));
        let r0 = scaled[0].floor() as u32;
        let g0 = scaled[1].floor() as u32;
        let b0 = scaled[2].floor() as u32;
        let r1 = (r0 + 1).min(self.size - 1);
        let g1 = (g0 + 1).min(self.size - 1);
        let b1 = (b0 + 1).min(self.size - 1);
        let fr = scaled[0] - r0 as f32;
        let fg = scaled[1] - g0 as f32;
        let fb = scaled[2] - b0 as f32;

        let c000 = self.at(r0, g0, b0);
        let c100 = self.at(r1, g0, b0);
        let c010 = self.at(r0, g1, b0);
        let c110 = self.at(r1, g1, b0);
        let c001 = self.at(r0, g0, b1);
        let c101 = self.at(r1, g0, b1);
        let c011 = self.at(r0, g1, b1);
        let c111 = self.at(r1, g1, b1);

        tetrahedral_interp(c000, c001, c010, c011, c100, c101, c110, c111, fr, fg, fb)
    }

    /// Trilinear interpolation — useful as a test baseline.
    pub fn sample_trilinear(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mapped = self.map_input(rgb);
        let size_f = self.size as f32;
        let scaled = mapped.map(|v| v * (size_f - 1.0));
        let r0 = scaled[0].floor() as u32;
        let g0 = scaled[1].floor() as u32;
        let b0 = scaled[2].floor() as u32;
        let r1 = (r0 + 1).min(self.size - 1);
        let g1 = (g0 + 1).min(self.size - 1);
        let b1 = (b0 + 1).min(self.size - 1);
        let fr = scaled[0] - r0 as f32;
        let fg = scaled[1] - g0 as f32;
        let fb = scaled[2] - b0 as f32;

        let c000 = self.at(r0, g0, b0);
        let c100 = self.at(r1, g0, b0);
        let c010 = self.at(r0, g1, b0);
        let c110 = self.at(r1, g1, b0);
        let c001 = self.at(r0, g0, b1);
        let c101 = self.at(r1, g0, b1);
        let c011 = self.at(r0, g1, b1);
        let c111 = self.at(r1, g1, b1);

        let c00 = lerp3(c000, c100, fr);
        let c10 = lerp3(c010, c110, fr);
        let c01 = lerp3(c001, c101, fr);
        let c11 = lerp3(c011, c111, fr);
        let c0 = lerp3(c00, c10, fg);
        let c1 = lerp3(c01, c11, fg);
        lerp3(c0, c1, fb)
    }

    pub fn apply(&self, rgb: [f32; 3], mix: f32) -> [f32; 3] {
        let mix = mix.clamp(0.0, 1.0);
        if mix < 1.0e-4 {
            return rgb;
        }
        let mapped = self.sample_tetrahedral(rgb);
        if mix >= 0.999 {
            mapped.map(|channel| channel.clamp(0.0, 1.0))
        } else {
            [
                (rgb[0] + (mapped[0] - rgb[0]) * mix).clamp(0.0, 1.0),
                (rgb[1] + (mapped[1] - rgb[1]) * mix).clamp(0.0, 1.0),
                (rgb[2] + (mapped[2] - rgb[2]) * mix).clamp(0.0, 1.0),
            ]
        }
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn add_weighted(acc: [f32; 3], sample: [f32; 3], weight: f32) -> [f32; 3] {
    [
        acc[0] + sample[0] * weight,
        acc[1] + sample[1] * weight,
        acc[2] + sample[2] * weight,
    ]
}

fn tetrahedral_interp(
    c000: [f32; 3],
    c001: [f32; 3],
    c010: [f32; 3],
    c011: [f32; 3],
    c100: [f32; 3],
    c101: [f32; 3],
    c110: [f32; 3],
    c111: [f32; 3],
    fr: f32,
    fg: f32,
    fb: f32,
) -> [f32; 3] {
    if fr >= fg {
        if fg >= fb {
            let mut out = add_weighted([0.0; 3], c000, 1.0 - fr);
            out = add_weighted(out, c100, fr - fg);
            out = add_weighted(out, c110, fg - fb);
            add_weighted(out, c111, fb)
        } else if fr >= fb {
            let mut out = add_weighted([0.0; 3], c000, 1.0 - fr);
            out = add_weighted(out, c100, fr - fb);
            out = add_weighted(out, c101, fb - fg);
            add_weighted(out, c111, fg)
        } else {
            let mut out = add_weighted([0.0; 3], c000, 1.0 - fb);
            out = add_weighted(out, c001, fb - fr);
            out = add_weighted(out, c101, fr - fg);
            add_weighted(out, c111, fg)
        }
    } else if fg >= fb {
        if fb >= fr {
            let mut out = add_weighted([0.0; 3], c000, 1.0 - fg);
            out = add_weighted(out, c010, fg - fb);
            out = add_weighted(out, c011, fb - fr);
            add_weighted(out, c111, fr)
        } else {
            let mut out = add_weighted([0.0; 3], c000, 1.0 - fg);
            out = add_weighted(out, c010, fg - fr);
            out = add_weighted(out, c110, fr - fb);
            add_weighted(out, c111, fb)
        }
    } else if fb >= fr {
        let mut out = add_weighted([0.0; 3], c000, 1.0 - fb);
        out = add_weighted(out, c001, fb - fg);
        out = add_weighted(out, c011, fg - fr);
        add_weighted(out, c111, fr)
    } else {
        let mut out = add_weighted([0.0; 3], c000, 1.0 - fb);
        out = add_weighted(out, c001, fb - fr);
        out = add_weighted(out, c101, fr - fg);
        add_weighted(out, c111, fg)
    }
}

fn parse_size_line(line: &str, line_no: usize) -> Result<u32, LutError> {
    let value = line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| LutError::InvalidLine {
            line: line_no,
            detail: "expected LUT_3D_SIZE <n>".into(),
        })?;
    value
        .parse::<u32>()
        .map_err(|_| LutError::InvalidLine {
            line: line_no,
            detail: format!("invalid LUT_3D_SIZE `{value}`"),
        })
}

fn parse_triplet_line(line: &str, keyword: &str, line_no: usize) -> Result<[f32; 3], LutError> {
    let mut parts = line.split_whitespace();
    let head = parts.next().unwrap_or("");
    if head != keyword {
        return Err(LutError::InvalidLine {
            line: line_no,
            detail: format!("expected {keyword} r g b"),
        });
    }
    let values: Result<Vec<f32>, _> = parts.take(3).map(str::parse).collect();
    let values = values.map_err(|_| LutError::InvalidLine {
        line: line_no,
        detail: format!("expected three floats after {keyword}"),
    })?;
    if values.len() != 3 {
        return Err(LutError::InvalidLine {
            line: line_no,
            detail: format!("expected three floats after {keyword}"),
        });
    }
    Ok([values[0], values[1], values[2]])
}

fn parse_rgb_line(line: &str, line_no: usize) -> Result<[f32; 3], LutError> {
    let values: Result<Vec<f32>, _> = line.split_whitespace().take(3).map(str::parse).collect();
    let values = values.map_err(|_| LutError::InvalidLine {
        line: line_no,
        detail: "expected three RGB floats".into(),
    })?;
    if values.len() != 3 {
        return Err(LutError::InvalidLine {
            line: line_no,
            detail: "expected three RGB floats".into(),
        });
    }
    Ok([values[0], values[1], values[2]])
}

fn parse_quoted_or_tail(line: &str, keyword: &str) -> Result<String, LutError> {
    let rest = line.strip_prefix(keyword).unwrap_or(line).trim();
    if rest.starts_with('"') {
        let end = rest[1..].find('"').map(|index| index + 2).unwrap_or(rest.len());
        Ok(rest[1..end - 1].to_string())
    } else if rest.is_empty() {
        Ok(String::new())
    } else {
        Ok(rest.to_string())
    }
}

pub fn parse_cube_file(path: &Path) -> Result<Lut3D, LutError> {
    Lut3D::from_cube_file(path)
}

pub fn resolve_lut(
    path: &str,
    embedded: Option<&EmbeddedLut3D>,
) -> Option<Lut3D> {
    if let Some(table) = embedded {
        return Lut3D::from_embedded(table).ok();
    }
    if path.is_empty() {
        return None;
    }
    Lut3D::from_cube_file(Path::new(path)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY_2: &str = "\
TITLE \"Identity\"
LUT_3D_SIZE 2
0.0 0.0 0.0
1.0 0.0 0.0
0.0 1.0 0.0
1.0 1.0 0.0
0.0 0.0 1.0
1.0 0.0 1.0
0.0 1.0 1.0
1.0 1.0 1.0
";

    const RED_BOOST_2: &str = "\
LUT_3D_SIZE 2
0.0 0.0 0.0
1.0 0.0 0.0
0.0 0.5 0.0
1.0 0.5 0.0
0.0 0.0 0.5
1.0 0.0 0.5
0.0 0.5 0.5
1.0 0.5 0.5
";

    #[test]
    fn parse_identity_cube() {
        let lut = Lut3D::from_cube_str(IDENTITY_2).unwrap();
        assert_eq!(lut.size, 2);
        assert_eq!(lut.title.as_deref(), Some("Identity"));
        assert_eq!(lut.table.len(), 8);
    }

    #[test]
    fn tetrahedral_hits_grid_corners() {
        let lut = Lut3D::from_cube_str(IDENTITY_2).unwrap();
        let black = lut.sample_tetrahedral([0.0, 0.0, 0.0]);
        assert!((black[0] - 0.0).abs() < 1.0e-4);
        let white = lut.sample_tetrahedral([1.0, 1.0, 1.0]);
        assert!((white[0] - 1.0).abs() < 1.0e-4);
        assert!((white[1] - 1.0).abs() < 1.0e-4);
        assert!((white[2] - 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn tetrahedral_mid_grey_on_identity() {
        let lut = Lut3D::from_cube_str(IDENTITY_2).unwrap();
        let mid = lut.sample_tetrahedral([0.5, 0.5, 0.5]);
        assert!((mid[0] - 0.5).abs() < 0.06);
        assert!((mid[1] - 0.5).abs() < 0.06);
        assert!((mid[2] - 0.5).abs() < 0.06);
    }

    #[test]
    fn mix_at_half_strength() {
        let lut = Lut3D::from_cube_str(RED_BOOST_2).unwrap();
        let input = [0.5, 0.5, 0.5];
        let full = lut.sample_tetrahedral(input);
        let half = lut.apply(input, 0.5);
        assert!((half[0] - (input[0] + (full[0] - input[0]) * 0.5)).abs() < 1.0e-4);
    }

    #[test]
    fn embedded_round_trip() {
        let lut = Lut3D::from_cube_str(IDENTITY_2).unwrap();
        let embedded = lut.to_embedded();
        let restored = Lut3D::from_embedded(&embedded).unwrap();
        assert_eq!(restored.table, lut.table);
    }

    #[test]
    fn sample_identity_cube_loads_from_disk() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples/luts/identity-2.cube");
        let lut = Lut3D::from_cube_file(&path).unwrap();
        assert_eq!(lut.size, 2);
        assert_eq!(lut.title.as_deref(), Some("Identity 2"));
    }

    #[test]
    fn rejects_1d_lut_marker() {
        let text = "LUT_1D_SIZE 32\n0.0\n1.0";
        let err = Lut3D::from_cube_str(text).unwrap_err();
        assert!(matches!(err, LutError::InvalidLine { .. }));
    }
}
