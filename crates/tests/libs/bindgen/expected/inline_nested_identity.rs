pub mod Left {
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Outer {
        pub tag: u32,
        pub Anonymous: Outer_0,
    }
    impl Default for Outer {
        fn default() -> Self {
            unsafe { core::mem::zeroed() }
        }
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Outer_0 {
        pub owner: *mut Outer,
        pub Anonymous: Outer_0_0,
    }
    impl Default for Outer_0 {
        fn default() -> Self {
            unsafe { core::mem::zeroed() }
        }
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub union Outer_0_0 {
        pub signed: i32,
        pub unsigned: u32,
    }
    impl Default for Outer_0_0 {
        fn default() -> Self {
            unsafe { core::mem::zeroed() }
        }
    }
}
pub mod Right {
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Outer {
        pub Anonymous: Outer_0,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Outer_0 {
        pub value: u64,
    }
}
