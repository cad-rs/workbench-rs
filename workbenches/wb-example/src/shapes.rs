//! The example Workbench's domain document: a list of shapes (serialized as JSON).
//! The platform does not know this type — it only touches the content through the
//! DocumentTypeDef codec functions.

use workbench_api::{DocumentContent, DocumentFormat, DocumentTypeDef, DocumentTypeId, ViewTypeId};

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

/// The file format: JSON with a `version` field. Files from before it had one
/// are version 1.
pub const FORMAT: DocumentFormat = DocumentFormat {
    oldest: 1,
    upgrades: &[add_version_field],
    version_of,
};

fn version_of(bytes: &[u8]) -> Result<u32, String> {
    #[derive(serde::Deserialize)]
    struct Header {
        version: Option<u32>,
    }
    let header: Header = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(header.version.unwrap_or(1))
}

/// 1 → 2: the file starts saying which version it is.
fn add_version_field(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let fields = doc.as_object_mut().ok_or("not a shape document")?;
    fields.insert("version".to_string(), 2.into());
    serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())
}

/// The registration of this document type.
pub fn document_type() -> DocumentTypeDef {
    DocumentTypeDef {
        id: DocumentTypeId::new("example.shapes"),
        title: "Shape Document".to_string(),
        extensions: vec!["shapes".to_string()],
        create_default: || Box::new(ShapeDoc::default()),
        serialize,
        deserialize,
        format: Some(FORMAT),
        open_view: Some(|_content: &dyn DocumentContent| {
            Some(ViewTypeId::new("example.canvas"))
        }),
        undo_hooks: None,
        source: None,
    }
}

/// Serialize (platform callback: unaware of the domain type, downcasts through Any).
pub fn serialize(content: &dyn DocumentContent) -> Result<Vec<u8>, String> {
    #[derive(serde::Serialize)]
    struct Saved<'a> {
        version: u32,
        #[serde(flatten)]
        doc: &'a ShapeDoc,
    }
    let doc = content
        .as_any()
        .downcast_ref::<ShapeDoc>()
        .ok_or("content is not a ShapeDoc")?;
    let saved = Saved {
        version: FORMAT.current(),
        doc,
    };
    serde_json::to_vec_pretty(&saved).map_err(|e| e.to_string())
}

/// Deserialize.
pub fn deserialize(bytes: &[u8]) -> Result<Box<dyn DocumentContent>, String> {
    let doc: ShapeDoc = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(Box::new(doc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use workbench_api::{DocumentId, DocumentService};

    /// A file as saved before the format had a version.
    const VERSION_1: &str = r#"{
        "name": "Old",
        "shapes": [{ "x": 1.0, "y": 2.0, "w": 3.0, "h": 4.0, "round": true, "hue": 5.0 }],
        "next_hue": 52.0
    }"#;

    fn open(bytes: &[u8]) -> (DocumentService, Result<DocumentId, String>) {
        let mut docs = DocumentService::new();
        docs.register_type(document_type()).unwrap();
        let type_id = DocumentTypeId::new("example.shapes");
        let opened = docs.open_bytes(&type_id, "part", None, bytes);
        (docs, opened)
    }

    #[test]
    fn a_file_from_before_versions_opens_and_saves_in_the_current_format() {
        let (docs, opened) = open(VERSION_1.as_bytes());
        let id = opened.unwrap();
        assert_eq!(docs.get(id).unwrap().upgraded_from, Some(1));
        let (name, shapes) = docs.read::<ShapeDoc, _>(id, |d| (d.name.clone(), d.shapes.len())).unwrap();
        assert_eq!((name.as_str(), shapes), ("Old", 1));

        let saved: serde_json::Value = serde_json::from_slice(&docs.encode(id).unwrap()).unwrap();
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["name"], "Old");
        assert_eq!(saved["shapes"][0]["hue"], 5.0);
    }

    #[test]
    fn a_saved_file_reopens_without_an_upgrade() {
        let (docs, opened) = open(VERSION_1.as_bytes());
        let saved = docs.encode(opened.unwrap()).unwrap();

        let (docs, reopened) = open(&saved);
        let id = reopened.unwrap();
        assert_eq!(docs.get(id).unwrap().upgraded_from, None);
        assert_eq!(docs.encode(id).unwrap(), saved);
    }

    #[test]
    fn a_file_from_a_newer_build_is_refused() {
        let (docs, opened) = open(br#"{ "version": 3, "name": "New", "shapes": [], "next_hue": 0 }"#);
        assert!(opened.unwrap_err().contains("update the application"));
        assert_eq!(docs.docs().count(), 0);
    }
}
