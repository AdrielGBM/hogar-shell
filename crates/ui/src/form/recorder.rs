//! Which fields a form being built has, for whatever applies that form when one of them moves.
//!
//! A form is built inside [`recording`], and every row built there registers its value with it; [`take`] hands what was registered to the button or debounce that applies the form, and the next row starts the next form. Outside a recording a row registers nothing, so a row built for something that is not such a form — an inspector that previews every change itself — is never counted as one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use telar::{Effect, ReadSignal, RwSignal, effect, signal};

/// What a form's fields came to: a revision that moves whenever one of them changes, and the effects that move it, which live exactly as long as whoever holds them.
pub struct Recorded {
    pub revision: ReadSignal<u64>,
    pub subscriptions: Vec<Effect>,
}

struct Recorder {
    revision: RwSignal<u64>,
    subscriptions: Vec<Effect>,
}

thread_local! {
    static RECORDING: RefCell<Option<Recorder>> = const { RefCell::new(None) };
    static OPEN: Cell<u32> = const { Cell::new(0) };
}

/// Builds `build` as one or more forms: each row built inside it is registered with the form it belongs to, up to the [`take`] that closes that form. Whatever no `take` claimed by the end goes with the recording.
pub fn recording<R>(build: impl FnOnce() -> R) -> R {
    OPEN.with(|open| open.set(open.get() + 1));
    let built = build();
    let outermost = OPEN.with(|open| {
        open.set(open.get() - 1);
        open.get() == 0
    });
    if outermost {
        RECORDING.with(|recording| recording.borrow_mut().take());
    }
    built
}

/// The fields registered since the last `take`, closing that form. `None` when there were none.
pub fn take() -> Option<Recorded> {
    RECORDING
        .with(|recording| recording.borrow_mut().take())
        .map(|recorder| Recorded {
            revision: recorder.revision.read_only(),
            subscriptions: recorder.subscriptions,
        })
}

/// Registers `value` as a field of the form being built, if one is.
pub fn record_field<T: Clone + PartialEq + 'static>(value: &RwSignal<T>) {
    if OPEN.with(Cell::get) == 0 {
        return;
    }
    let watched = value.read_only();
    RECORDING.with(|recording| {
        let mut recording = recording.borrow_mut();
        let recorder = recording.get_or_insert_with(|| Recorder {
            revision: signal(0u64),
            subscriptions: Vec::new(),
        });
        let revision = recorder.revision;
        // An effect runs once as it is registered, and that run is the field being seeded, not a change to it.
        let seeded = Cell::new(false);
        recorder.subscriptions.push(effect(move || {
            let _ = watched.get();
            if seeded.replace(true) {
                revision.set(revision.peek() + 1);
            }
        }));
    });
}

/// Binds a `String` field to the index a picker speaks in, both ways, and registers it: a value written back from elsewhere moves the picker with it.
pub fn option_index(value: RwSignal<String>, options: Rc<[&'static str]>) -> RwSignal<u32> {
    record_field(&value);
    let index_of = move |current: &str| {
        options
            .iter()
            .position(|option| *option == current)
            .unwrap_or(0) as u32
    };
    let picked = signal(index_of(&value.peek()));
    let follow = value.read_only();
    effect(move || {
        let at = index_of(&follow.get());
        if picked.peek() != at {
            picked.set(at);
        }
    });
    picked
}

/// Writes the option at `at` to the field it came from; picking what is already there changes nothing.
pub fn pick_option(value: &RwSignal<String>, options: &[&'static str], at: u32) {
    let Some(next) = options.get(at as usize) else {
        return;
    };
    if value.peek() != *next {
        value.set(next.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seeding a form is not a change to it, either field counts, and a form ends at its `take`.
    #[test]
    fn a_recorded_form_counts_changes_and_not_its_seeding() {
        telar::reset_runtime();
        let (name, filled) = (signal("nord".to_string()), signal(false));
        let recorded = recording(|| {
            record_field(&name);
            record_field(&filled);
            take().expect("two fields were recorded")
        });
        assert_eq!(recorded.subscriptions.len(), 2);
        assert_eq!(
            recorded.revision.peek(),
            0,
            "drawing the form is not editing it"
        );
        name.set("rose-pine".to_string());
        filled.set(true);
        assert_eq!(recorded.revision.peek(), 2);
        assert!(take().is_none(), "the next form starts empty");
    }

    /// A row built outside a recording is nobody's field: nothing waits for a `take` that would hand it to the next form built.
    #[test]
    fn a_field_outside_a_recording_is_not_recorded() {
        telar::reset_runtime();
        record_field(&signal(1u32));
        assert!(take().is_none());
        recording(|| record_field(&signal(2u32)));
        assert!(
            take().is_none(),
            "what no form claimed went with its recording"
        );
    }
}
