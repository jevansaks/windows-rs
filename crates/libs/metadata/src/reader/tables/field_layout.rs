use super::*;

impl FieldLayout<'_> {
    pub fn offset(&self) -> u32 {
        self.usize(0).try_into().unwrap()
    }
}
