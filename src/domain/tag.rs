use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TagId(pub Uuid);

impl TagId {
    pub fn new() -> Self {
        TagId(Uuid::new_v4())
    }
}

#[derive(Debug, Clone)]
pub struct Tag {
    pub id: TagId,
    pub name: String,
}
