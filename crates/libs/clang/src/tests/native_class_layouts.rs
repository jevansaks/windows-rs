use super::*;

#[test]
fn pointer_only_native_class_layouts_match_x64_and_x86() {
    helpers::ensure_libclang();

    for (target, pointer_size, path_size, path_align) in [
        ("x86_64-pc-windows-msvc", 8, 24, 8),
        ("i686-pc-windows-msvc", 4, 12, 4),
    ] {
        let snapshot = extract(
            [Input::new("geometry.hpp", SOURCE)],
            &[
                "-x",
                "c++",
                "-fms-extensions",
                &format!("--target={target}"),
            ],
        )
        .unwrap();

        assert_layout(&snapshot, "Point", 8, 4, &[("X", 0, 4, 4), ("Y", 32, 4, 4)]);
        assert_layout(
            &snapshot,
            "Rect",
            16,
            4,
            &[
                ("X", 0, 4, 4),
                ("Y", 32, 4, 4),
                ("Width", 64, 4, 4),
                ("Height", 96, 4, 4),
            ],
        );
        assert_layout(
            &snapshot,
            "Size",
            8,
            4,
            &[("Width", 0, 4, 4), ("Height", 32, 4, 4)],
        );
        assert_layout(
            &snapshot,
            "PointF",
            8,
            4,
            &[("X", 0, 4, 4), ("Y", 32, 4, 4)],
        );
        assert_layout(
            &snapshot,
            "RectF",
            16,
            4,
            &[
                ("X", 0, 4, 4),
                ("Y", 32, 4, 4),
                ("Width", 64, 4, 4),
                ("Height", 96, 4, 4),
            ],
        );
        assert_layout(
            &snapshot,
            "PathData",
            path_size,
            path_align,
            &[
                ("Count", 0, 4, 4),
                ("Points", pointer_size * 8, pointer_size, pointer_size),
                ("Types", pointer_size * 16, pointer_size, pointer_size),
            ],
        );

        for name in ["Empty", "PrivateGeometry", "VirtualGeometry"] {
            assert!(
                snapshot
                    .facts
                    .iter()
                    .filter(|fact| fact.name == name)
                    .all(|fact| !snapshot
                        .pointer_only_class_layouts
                        .contains_key(&fact.origin)),
                "{name} unexpectedly had a pointer-only layout for {target}"
            );
        }
    }
}

fn assert_layout(
    snapshot: &Snapshot,
    name: &str,
    size: i64,
    align: i64,
    expected_fields: &[(&str, i64, i64, i64)],
) {
    let facts: Vec<_> = snapshot
        .facts
        .iter()
        .filter(|fact| fact.name == name)
        .collect();
    assert!(!facts.is_empty(), "{name} was not extracted");
    assert!(
        facts
            .iter()
            .all(|fact| matches!(fact.data, FactData::Unsupported { .. })),
        "{name} should remain unsupported outside selected header planning"
    );
    for fact in facts {
        let FactData::Record {
            base,
            fields,
            size: actual_size,
            align: actual_align,
            packing,
            alignment,
            union,
        } = &snapshot.pointer_only_class_layouts[&fact.origin]
        else {
            panic!("{name} did not retain a pointer-only record layout");
        };
        assert!(base.is_none());
        assert_eq!(*actual_size, size);
        assert_eq!(*actual_align, align);
        assert_eq!(*packing, None);
        assert_eq!(*alignment, None);
        assert!(!union);
        assert_eq!(
            fields
                .iter()
                .map(|field| (field.name.as_str(), field.offset, field.align, field.size))
                .collect::<Vec<_>>(),
            expected_fields
        );
    }
}

const SOURCE: &str = r#"
    typedef unsigned char BYTE;
    typedef int INT;
    typedef float REAL;

    namespace Gdiplus {
        class Point;
        class Rect;
        class Size;
        class PointF;
        class RectF;
        class PathData;

        class Point {
        public:
            Point();
            Point(const Point& other);
            INT X;
            INT Y;
        };

        class Rect {
        public:
            Rect();
            Rect(const Rect& other);
            Rect Clone() const;
            INT X;
            INT Y;
            INT Width;
            INT Height;
        };

        class Size {
        public:
            Size();
            Size(const Size& other);
            INT Width;
            INT Height;
        };

        class PointF {
        public:
            PointF();
            PointF(const PointF& other);
            bool Equals(const PointF& other) const;
            REAL X;
            REAL Y;
        };

        class RectF {
        public:
            RectF();
            RectF Clone() const;
            REAL X;
            REAL Y;
            REAL Width;
            REAL Height;
        };

        class PathData {
        public:
            PathData();
            ~PathData();
        private:
            PathData(const PathData& other);
            PathData& operator=(const PathData& other);
        public:
            INT Count;
            PointF* Points;
            BYTE* Types;
        };

        class Empty {};
        class PrivateGeometry { int Hidden; public: int Visible; };
        class VirtualGeometry { public: virtual void Reset(); int Value; };
    }
"#;
