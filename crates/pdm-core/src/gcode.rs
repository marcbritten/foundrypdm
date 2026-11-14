//! Read-only G-code parser for MFG/CAM previews (ToDoMFGCAM: "G-code parsing and read-only
//! visualization"). Extracts a summary + a 2.5D toolpath so the UI can draw it. It does NOT
//! execute or simulate: it reads G0/G1/G2/G3 moves, T/M6 tool changes, M3/M5 spindle,
//! G90/G91 absolute/relative, and M30/M2 program end. Pure and total (never panics on input).
//! Arcs are flattened to their endpoint (a read-only-preview approximation). G-code is not
//! assumed portable between machines — the summary records what it saw, not how to run it.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct GcodeBoundingBox {
    pub xmin: f64,
    pub xmax: f64,
    pub ymin: f64,
    pub ymax: f64,
    pub zmin: f64,
    pub zmax: f64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct GcodeToolChange {
    pub line: u32,
    pub tool: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct GcodeSummary {
    pub line_count: usize,
    pub toolpath: Vec<[f64; 3]>,
    pub tools: Vec<u32>,
    pub tool_changes: Vec<GcodeToolChange>,
    pub spindle_on: bool,
    pub bounding_box: GcodeBoundingBox,
    pub rapid_count: usize,
    pub feed_count: usize,
    pub arc_count: usize,
    pub ends_with_program_end: bool,
    pub warnings: Vec<String>,
}

/// Parse G-code text into a summary + toolpath.
pub fn parse_gcode(text: &str) -> GcodeSummary {
    let mut line_count = 0usize;
    let mut toolpath: Vec<[f64; 3]> = Vec::new();
    let mut tools: Vec<u32> = Vec::new();
    let mut tool_changes: Vec<GcodeToolChange> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut spindle_on = false;
    let mut rapid = 0usize;
    let mut feed = 0usize;
    let mut arcs = 0usize;
    let mut program_end = false;
    let mut x = 0.0;
    let mut y = 0.0;
    let mut z = 0.0;
    let mut relative = false;
    let mut pending_tool: Option<u32> = None;
    let mut bbox: Option<GcodeBoundingBox> = None;

    for raw in text.lines() {
        line_count += 1;
        let line_no = line_count as u32;
        let code = strip_comment(raw);
        if code.trim().is_empty() {
            continue;
        }
        let mut had_move = false;
        let mut had_xyz = false;
        let mut nx = x;
        let mut ny = y;
        let mut nz = z;
        for (letter, value) in parse_words(&code) {
            match letter {
                'G' => match value as u32 {
                    0 => {
                        rapid += 1;
                        had_move = true;
                    }
                    1 => {
                        feed += 1;
                        had_move = true;
                    }
                    2 | 3 => {
                        arcs += 1;
                        had_move = true;
                    }
                    // Planes (G17/18/19), units (G20/21), cutter comp (G40-43): recognized, no-op.
                    17 | 18 | 19 | 20 | 21 | 40 | 41 | 42 | 43 => {}
                    90 => relative = false,
                    91 => relative = true,
                    other => warnings.push(format!("line {line_no}: unsupported G{other}")),
                },
                'X' => {
                    nx = if relative { x + value } else { value };
                    had_xyz = true;
                }
                'Y' => {
                    ny = if relative { y + value } else { value };
                    had_xyz = true;
                }
                'Z' => {
                    nz = if relative { z + value } else { value };
                    had_xyz = true;
                }
                'T' => pending_tool = Some(value as u32),
                'M' => match value as u32 {
                    3 => spindle_on = true,
                    5 => spindle_on = false,
                    6 => {
                        if let Some(t) = pending_tool {
                            if !tools.contains(&t) {
                                tools.push(t);
                            }
                            tool_changes.push(GcodeToolChange {
                                line: line_no,
                                tool: t,
                            });
                        }
                    }
                    2 | 30 => program_end = true,
                    _ => {}
                },
                // S (speed), F (feed), I/J/K (arc centers), R (radius), L/P: read, not tracked.
                _ => {}
            }
        }
        if had_move && had_xyz {
            x = nx;
            y = ny;
            z = nz;
            toolpath.push([x, y, z]);
            let b = bbox.unwrap_or(GcodeBoundingBox {
                xmin: x,
                xmax: x,
                ymin: y,
                ymax: y,
                zmin: z,
                zmax: z,
            });
            bbox = Some(GcodeBoundingBox {
                xmin: b.xmin.min(x),
                xmax: b.xmax.max(x),
                ymin: b.ymin.min(y),
                ymax: b.ymax.max(y),
                zmin: b.zmin.min(z),
                zmax: b.zmax.max(z),
            });
        }
    }

    GcodeSummary {
        line_count,
        toolpath,
        tools,
        tool_changes,
        spindle_on,
        bounding_box: bbox.unwrap_or_default(),
        rapid_count: rapid,
        feed_count: feed,
        arc_count: arcs,
        ends_with_program_end: program_end,
        warnings,
    }
}

/// Remove a G-code comment: an inline `(...)` group, or a trailing `;`/`%` to end-of-line.
fn strip_comment(line: &str) -> String {
    let mut out = String::new();
    let mut in_paren = false;
    for c in line.chars() {
        match c {
            '(' => in_paren = true,
            ')' => in_paren = false,
            ';' | '%' if !in_paren => break,
            c if !in_paren => out.push(c),
            _ => {}
        }
    }
    out
}

/// Parse G-code "words" (a single uppercase letter + a signed decimal) in order, e.g.
/// "G1 X10.5 Y-2 T3" -> [('G',1.0), ('X',10.5), ('Y',-2.0), ('T',3.0)].
fn parse_words(code: &str) -> Vec<(char, f64)> {
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_uppercase() {
            let mut num = String::new();
            let mut j = i + 1;
            if j < chars.len() && chars[j] == '-' {
                num.push('-');
                j += 1;
            }
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                num.push(chars[j]);
                j += 1;
            }
            if !num.is_empty()
                && let Ok(v) = num.parse::<f64>()
            {
                out.push((c, v));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_simple_profile() {
        let g =
            "G21\nG90\nT1 M6\nM3 S12000\nG0 X0 Y0 Z5\nG1 X10 F150\nG1 X10 Y5\nG0 X0 Y0\nM5\nM30";
        let s = parse_gcode(g);
        assert_eq!(s.line_count, 10);
        assert_eq!(s.rapid_count, 2, "two G0 rapid moves");
        assert_eq!(s.feed_count, 2, "two G1 feed moves");
        assert_eq!(s.tools, vec![1]);
        assert_eq!(s.tool_changes, vec![GcodeToolChange { line: 3, tool: 1 }]);
        assert!(!s.spindle_on, "spindle off after M5");
        assert!(s.ends_with_program_end, "M30 program-end detected");
        assert_eq!(s.bounding_box.xmin, 0.0);
        assert_eq!(s.bounding_box.xmax, 10.0);
        assert_eq!(s.bounding_box.ymax, 5.0);
        assert_eq!(s.toolpath.len(), 4, "one toolpath point per move line");
        assert!(
            s.warnings.is_empty(),
            "no warnings for recognized words: {:?}",
            s.warnings
        );
    }

    #[test]
    fn empty_gcode_is_safe() {
        let s = parse_gcode("");
        assert_eq!(s.line_count, 0);
        assert!(s.toolpath.is_empty());
        assert!(
            s.bounding_box.xmin == 0.0,
            "empty bbox is zeros, not infinities"
        );
    }

    #[test]
    fn strips_comments_and_counts_arcs() {
        let g = "(header comment)\nG0 X0 Y0\nG2 X5 I2.5 ; quarter arc\nM2";
        let s = parse_gcode(g);
        assert_eq!(s.arc_count, 1, "G2 counted as an arc");
        assert!(s.ends_with_program_end, "M2 is a program end");
        assert!(s.warnings.is_empty());
    }
}
