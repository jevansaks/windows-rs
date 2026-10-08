use super::*;

impl std::fmt::Debug for Assembly<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_tuple("Assembly").field(&self.0).finish()
    }
}

impl<'a> Assembly<'a> {
    pub fn hash_algorithm(&self) -> u32 {
        self.usize(0).try_into().unwrap()
    }

    pub fn version(&self) -> (u16, u16, u16, u16) {
        let version = self.u64(1);
        (
            version as u16,
            (version >> 16) as u16,
            (version >> 32) as u16,
            (version >> 48) as u16,
        )
    }

    pub fn flags(&self) -> AssemblyFlags {
        AssemblyFlags(self.usize(2).try_into().unwrap())
    }

    pub fn public_key(&self) -> &'a [u8] {
        self.file().blob(self.pos(), Self::TABLE, 3)
    }

    pub fn name(&self) -> &'a str {
        self.str(4)
    }

    pub fn culture(&self) -> &'a str {
        self.str(5)
    }
}
