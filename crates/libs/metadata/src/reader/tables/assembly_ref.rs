use super::*;

impl std::fmt::Debug for AssemblyRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_tuple("AssemblyRef").field(&self.0).finish()
    }
}

impl<'a> AssemblyRef<'a> {
    pub fn version(&self) -> (u16, u16, u16, u16) {
        let version = self.u64(0);
        (
            version as u16,
            (version >> 16) as u16,
            (version >> 32) as u16,
            (version >> 48) as u16,
        )
    }

    pub fn flags(&self) -> AssemblyFlags {
        AssemblyFlags(self.usize(1).try_into().unwrap())
    }

    pub fn public_key_or_token(&self) -> &'a [u8] {
        self.file().blob(self.pos(), Self::TABLE, 2)
    }

    pub fn name(&self) -> &'a str {
        self.str(3)
    }

    pub fn culture(&self) -> &'a str {
        self.str(4)
    }

    pub fn hash_value(&self) -> &'a [u8] {
        self.file().blob(self.pos(), Self::TABLE, 5)
    }
}
