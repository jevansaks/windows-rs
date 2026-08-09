use super::*;

impl std::fmt::Debug for MethodImpl<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_tuple("MethodImpl").field(&self.0).finish()
    }
}

impl<'a> MethodImpl<'a> {
    pub fn class(&self) -> TypeDef<'a> {
        self.row(0)
    }

    pub fn body(&self) -> MethodDefOrRef<'a> {
        self.decode(1)
    }

    pub fn declaration(&self) -> MethodDefOrRef<'a> {
        self.decode(2)
    }
}
