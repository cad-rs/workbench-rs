//! 示例 Workbench 的领域文档：图形列表（JSON 序列化）。
//! 平台不感知此类型——它只通过 DocumentTypeDef 的编解码函数与内容交互。

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
            name: "图形文档".to_string(),
            shapes: Vec::new(),
            next_hue: 12.0,
        }
    }
}

impl ShapeDoc {
    /// 确定性的“伪随机”添加：色相递进 + 位置按数量轮转，避免引入随机库。
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
            round: n % 3 == 0,
            hue,
        });
        self.shapes.len() - 1
    }

    /// 追加 n 个图形（后台批量生成用），返回新增起始索引。
    pub fn append(&mut self, n: usize) -> usize {
        let start = self.shapes.len();
        for _ in 0..n {
            self.add_shape();
        }
        start
    }
}

/// 序列化（平台回调：不感知领域类型，通过 Any 下溯）。
pub fn serialize(content: &dyn workbench_api::DocumentContent) -> Result<Vec<u8>, String> {
    let doc = content
        .as_any()
        .downcast_ref::<ShapeDoc>()
        .ok_or("内容不是 ShapeDoc")?;
    serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())
}

/// 反序列化。
pub fn deserialize(bytes: &[u8]) -> Result<Box<dyn workbench_api::DocumentContent>, String> {
    let doc: ShapeDoc = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(Box::new(doc))
}
