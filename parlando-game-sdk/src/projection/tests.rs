use super::*;
use crate::{encode_compact, GameValue};
use serde::Serialize;

#[derive(Serialize, GameValue)]
struct View<'a> {
    skip: &'a str,
    counts: [u64; 2],
    detail: Option<Detail<'a>>,
}

#[derive(Serialize, GameValue)]
struct Detail<'a> {
    floor: u64,
    actions: Vec<Action<'a>>,
}

#[derive(Serialize, GameValue)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Action<'a> {
    Wait,
    Move { destination: &'a str },
}

#[derive(Serialize, GameValue)]
enum Unit {
    First,
    Second,
}

/// Verifies direct reads of nested options and variant labels, with wire-order traversal.
#[test]
fn reads_nested_slots_and_enum_labels() -> Result<()> {
    let schema = Arc::new(View::schema());
    let view = View {
        skip: "unused",
        counts: [3, 5],
        detail: Some(Detail {
            floor: 4,
            actions: vec![
                Action::Wait,
                Action::Move {
                    destination: "attic",
                },
            ],
        }),
    };
    let value = ComponentValue::from_schema_bytes(encode_compact(&view).unwrap(), schema.clone())?;
    let actions = Selection::EnumLabel("action");
    let detail = Selection::Fields(&[
        ("actions", Selection::List(&actions)),
        ("floor", Selection::U64("floor")),
    ]);
    let plan = Projection::resolve(
        schema,
        &Selection::Fields(&[
            ("detail", Selection::Optional(&detail)),
            (
                "counts",
                Selection::Tuple(&[(1, Selection::U64("b")), (0, Selection::U64("a"))]),
            ),
        ]),
    )?;
    let mut got = Vec::new();
    plan.read(&value, |key, value| {
        got.push((key, value));
        Ok(())
    })?;
    assert_eq!(
        got,
        [
            ("a", ProjectedValue::U64(3)),
            ("b", ProjectedValue::U64(5)),
            ("floor", ProjectedValue::U64(4)),
            ("action", ProjectedValue::EnumLabel("wait")),
            ("action", ProjectedValue::EnumLabel("move")),
        ]
    );
    let absent = View {
        detail: None,
        ..view
    };
    let value = ComponentValue::from_schema_bytes(
        encode_compact(&absent).unwrap(),
        Arc::new(View::schema()),
    )?;
    let mut emitted = 0;
    plan.read(&value, |_, _| {
        emitted += 1;
        Ok(())
    })?;
    assert_eq!(emitted, 2);
    Ok(())
}

/// Tests the ordinal-only representation and reports corrupt or unrelated inputs.
#[test]
fn checks_declared_identity_and_enum_shape() -> Result<()> {
    let schema = Arc::new(Unit::schema());
    let plan = Projection::resolve(schema.clone(), &Selection::EnumLabel(()))?;
    for (unit, label) in [(Unit::First, "First"), (Unit::Second, "Second")] {
        let value =
            ComponentValue::from_schema_bytes(encode_compact(&unit).unwrap(), schema.clone())?;
        plan.read(&value, |(), value| {
            assert_eq!(value, ProjectedValue::EnumLabel(label));
            Ok(())
        })?;
    }
    for bytes in [vec![2], vec![0xff], vec![0, 0], vec![0x91, 0]] {
        let value = ComponentValue::from_schema_bytes(bytes, schema.clone())?;
        assert!(plan.read(&value, |(), _| Ok(())).is_err());
    }
    let mut reordered = Unit::schema();
    if let Schema::Enum { variants, .. } = &mut reordered {
        variants.reverse();
    }
    let wrong = ComponentValue::from_schema_bytes(vec![0], Arc::new(reordered.clone()))?;
    assert!(plan.read(&wrong, |(), _| Ok(())).is_err());
    Projection::resolve(Arc::new(reordered), &Selection::EnumLabel(()))?.read(
        &wrong,
        |(), value| {
            assert_eq!(value, ProjectedValue::EnumLabel("Second"));
            Ok(())
        },
    )?;
    let named = ComponentValue::from_typed(&Unit::First)?;
    assert!(plan.read(&named, |(), _| Ok(())).is_err());
    Ok(())
}

/// Checks selection-time errors and confirms skipped values still need valid structure.
#[test]
fn rejects_mismatched_selections_and_truncated_slots() -> Result<()> {
    let schema = Arc::new(Schema::Tuple(vec![Schema::U64, Schema::String]));
    for selection in [
        Selection::Tuple(&[(0, Selection::String(()))]),
        Selection::Tuple(&[(2, Selection::U64(()))]),
        Selection::Tuple(&[(0, Selection::U64(())), (0, Selection::U64(()))]),
    ] {
        assert!(Projection::resolve(schema.clone(), &selection).is_err());
    }
    let plan = Projection::resolve(
        schema.clone(),
        &Selection::Tuple(&[(0, Selection::U64(()))]),
    )?;
    assert!(Projection::resolve(
        Arc::new(View::schema()),
        &Selection::Fields(&[("missing", Selection::U64(()))])
    )
    .is_err());
    for bytes in [
        vec![0x91, 0],
        vec![0x92, 0, 0xd9, 2, b'x'],
        vec![0x92, 0xff, 0xa0],
    ] {
        let value = ComponentValue::from_schema_bytes(bytes, schema.clone())?;
        assert!(plan.read(&value, |(), _| Ok(())).is_err());
    }
    Ok(())
}

/// Selected strings and numbers follow the SDK's scalar validation rules.
#[test]
fn validates_selected_scalars() -> Result<()> {
    let schema = Arc::new(Schema::Tuple(vec![
        Schema::Bool,
        Schema::I64,
        Schema::F64,
        Schema::String,
    ]));
    let plan = Projection::resolve(
        schema.clone(),
        &Selection::Tuple(&[
            (0, Selection::Bool(0)),
            (1, Selection::I64(1)),
            (2, Selection::F64(2)),
            (3, Selection::String(3)),
        ]),
    )?;
    let string = vec![0xa2, b'o', b'k'];
    let float = [vec![0xcb], 1.25_f64.to_be_bytes().to_vec()].concat();
    let prefix = [vec![0x94, 0xc3, 4], float].concat();
    let valid =
        ComponentValue::from_schema_bytes([prefix.clone(), string].concat(), schema.clone())?;
    let mut actual = Vec::new();
    plan.read(&valid, |slot, value| {
        actual.push((slot, value));
        Ok(())
    })?;
    assert_eq!(
        actual,
        [
            (0, ProjectedValue::Bool(true)),
            (1, ProjectedValue::I64(4)),
            (2, ProjectedValue::F64(1.25)),
            (3, ProjectedValue::String("ok")),
        ]
    );
    for bytes in [
        [prefix, vec![0xa1, 0xff]].concat(),
        [
            vec![0x94, 0xc3, 4, 0xcb],
            f64::NAN.to_be_bytes().to_vec(),
            vec![0xa0],
        ]
        .concat(),
    ] {
        let value = ComponentValue::from_schema_bytes(bytes, schema.clone())?;
        assert!(plan.read(&value, |_, _| Ok(())).is_err());
    }
    Ok(())
}

/// A policy may retain borrowed leaves for the input lifetime and reject a partial read.
#[test]
fn borrows_text_and_propagates_policy_errors() -> Result<()> {
    #[derive(GameValue)]
    struct Scalars<'a> {
        enabled: bool,
        delta: i16,
        weight: f64,
        name: &'a str,
    }

    let schema = Arc::new(Scalars::schema());
    let selection = Selection::Fields(&[
        ("name", Selection::String(3)),
        ("weight", Selection::F64(2)),
        ("delta", Selection::I64(1)),
        ("enabled", Selection::Bool(0)),
    ]);
    let plan = Projection::resolve(schema.clone(), &selection)?;
    let bytes = encode_compact(&Scalars {
        enabled: true,
        delta: -2,
        weight: 0.5,
        name: "west",
    })
    .unwrap();
    let input = ComponentValue::from_schema_bytes(bytes, schema)?;
    let mut leaves = Vec::new();
    plan.read(&input, |key, value| {
        leaves.push((key, value));
        Ok(())
    })?;
    assert_eq!(
        leaves,
        [
            (0, ProjectedValue::Bool(true)),
            (1, ProjectedValue::I64(-2)),
            (2, ProjectedValue::F64(0.5)),
            (3, ProjectedValue::String("west")),
        ]
    );
    let ProjectedValue::String(label) = leaves[3].1 else {
        unreachable!()
    };
    assert!(input
        .as_messagepack()
        .as_ptr_range()
        .contains(&label.as_ptr()));
    let mut calls = 0;
    let err = plan
        .read(&input, |_, _| {
            calls += 1;
            anyhow::bail!("policy stopped")
        })
        .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(err.to_string(), "policy stopped");
    Ok(())
}
