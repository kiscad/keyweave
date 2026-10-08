#![allow(clippy::unwrap_used)]
use keyweave_engine::{ConfigSnapshot, Engine, Mapping};
use keyweave_linux::{events::*, keys::*};
use keyweave_types::*;
use std::sync::Arc;
#[test]
fn native_frame_normalizes_remaps_and_encodes_while_repeat_and_old_up_keep_their_bindings() {
    let key = |name: &str| name.parse::<KeyId>().unwrap();
    let snapshot = |generation, to: &str| {
        Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(generation),
            name: "native pipeline".into(),
            simple: vec![Mapping {
                id: "a".into(),
                from: key("a"),
                to: vec![key(to).into()],
                allow_extra_modifiers: true,
                extras: Default::default(),
            }],
            complex: vec![],
        })
    };
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::CanonicalEvdev);
    let mut decoder = FrameDecoder::new();
    let mut encoded = vec![];
    for (seq, value) in [(1, 1), (2, 2), (3, 0)] {
        if seq == 2 {
            engine.activate(snapshot(2, "c")).unwrap();
        }
        let raw = [
            Event::output(EV_KEY, 30, value),
            Event::output(EV_SYN, SYN_REPORT, 0),
        ];
        let bytes: Vec<_> = raw
            .into_iter()
            .flat_map(|event| encode(Layout::Time64, event).unwrap())
            .collect();
        let frames = decoder
            .ingest(&decode_batch(Layout::Time64, &bytes).unwrap())
            .unwrap();
        let Notice::Frame(events) = &frames[0] else {
            panic!("not a frame")
        };
        let NormalizedKey::Event { identity, edge } = normalizer.process(events[0], None).unwrap()
        else {
            panic!("missing saved identity")
        };
        let step = engine
            .handle(InputEnvelope {
                seq,
                at_ns: MonoTime(seq),
                device: DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                provenance: Provenance::Physical,
                event: InputEvent::Key {
                    key: identity.key,
                    edge,
                },
            })
            .unwrap();
        let keys: Vec<_> = step
            .outputs
            .iter()
            .map(|output| NativeKey {
                code: output_code(output.key).unwrap(),
                value: match output.edge {
                    KeyEdge::Down => 1,
                    KeyEdge::Up => 0,
                    KeyEdge::Repeat => 2,
                },
            })
            .collect();
        if value == 2 {
            assert!(keys.is_empty());
        }
        encoded.extend(
            decode_batch(
                Layout::Time64,
                &output_frame(Layout::Time64, &keys).unwrap(),
            )
            .unwrap(),
        );
    }
    assert_eq!(
        encoded,
        [
            Event::output(EV_KEY, 48, 1),
            Event::output(EV_SYN, SYN_REPORT, 0),
            Event::output(EV_KEY, 48, 0),
            Event::output(EV_SYN, SYN_REPORT, 0)
        ]
    );
    assert_eq!(engine.state().bindings, 0);
    assert_eq!(normalizer.pressed_count(), 0);
}
