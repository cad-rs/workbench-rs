//! The example Workbench's domain document: a list of shapes (serialized as JSON).
//! The platform does not know this type — it only touches the content through the
//! DocumentTypeDef codec functions.

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Shape {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub round: bool,
    pub hue: f32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ShapeDoc {
    pub name: String,
    pub shapes: Vec<Shape>,
    pub next_hue: f32,
}

impl Default for ShapeDoc {
    fn default() -> Self {
        Self {
            name: "Shape Document".to_string(),
            shapes: Vec::new(),
            next_hue: 12.0,
        }
    }
}

impl ShapeDoc {
    /// A deterministic "pseudo-random" add: the hue steps forward and the position rotates
    /// with the count, so no random-number crate is needed.
    pub fn add_shape(&mut self) -> usize {
        let n = self.shapes.len();
        let hue = self.next_hue;
        self.next_hue = (self.next_hue + 47.0) % 360.0;
        let x = 40.0 + ((n * 67) % 360) as f32;
        let y = 40.0 + ((n * 131) % 220) as f32;
        self.shapes.push(Shape {
            x,
            y,
            w: 46.0 + ((n * 29) % 60) as f32,
            h: 34.0 + ((n * 53) % 50) as f32,
            round: n.is_multiple_of(3),
            hue,
        });
        self.shapes.len() - 1
    }

    /// Append n shapes (used by the background batch generation); returns the index of the
    /// first new one.
    pub fn append(&mut self, n: usize) -> usize {
        let start = self.shapes.len();
        for _ in 0..n {
            self.add_shape();
        }
        start
    }
}

/// Serialize (platform callback: unaware of the domain type, downcasts through Any).
pub fn serialize(content: &dyn workbench_api::DocumentContent) -> Result<Vec<u8>, String> {
    let doc = content
        .as_any()
        .downcast_ref::<ShapeDoc>()
        .ok_or("content is not a ShapeDoc")?;
    serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())
}

/// Deserialize.
pub fn deserialize(bytes: &[u8]) -> Result<Box<dyn workbench_api::DocumentContent>, String> {
    let doc: ShapeDoc = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(Box::new(doc))
}
