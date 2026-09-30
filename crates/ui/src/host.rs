//! What a module is told about the place it is built into, instead of reading it from ambient globals: a Rust builder takes the [`Host`] as its argument, and a parameterless `.rsx` entrypoint reads it with [`Host::current`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Once};

use platform_wayland::EventSender;

use telar::{Color, LayoutError};

use config::{Config, Edge, ModuleOptions, ModuleOverride, ResolvedShape};

use crate::chrome::Chrome;

use crate::descriptor::FieldDef;

/// Which placed instance of a module is being built.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(Arc<str>);

impl InstanceId {
    pub fn new(id: &str) -> Self {
        Self(Arc::from(id))
    }

    /// The id a module's state is kept under when a chip, IPC and a keybind all have to reach the same state, which they name by module.
    pub fn of_module(module: &str) -> Self {
        Self::new(module)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The instance a build speaks for: whose state it keeps, which module it is, and what its entry in the layout says over that module's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub id: InstanceId,
    pub module: Arc<str>,
    /// The instance's own `options`, laid over its module's by [`Instance::options`] and [`Instance::presentation`] (TA-2).
    pub options: Arc<toml::Table>,
}

impl Instance {
    pub fn new(id: InstanceId, module: &str, options: toml::Table) -> Self {
        Self {
            id,
            module: Arc::from(module),
            options: Arc::new(options),
        }
    }

    /// A module reached by its id with no instance of the layout speaking for it — a preview, a dashboard card of another module, a panel of a module placed nowhere. Its defaults are its whole answer.
    pub fn of_module(module: &str) -> Self {
        Self::new(InstanceId::of_module(module), module, toml::Table::new())
    }

    /// The section `T`, with this instance's options over it when `T` is its module's own options type — the first its descriptor names. Another module's section is read as it is: an option is declared once, on the module that owns it.
    pub fn options<T: ModuleOptions>(&self, config: &Config) -> T {
        let own = crate::descriptor::find(&self.module)
            .and_then(|descriptor| descriptor.options.first())
            .is_some_and(|own| own.section == T::SECTION);
        if own {
            T::with_options(config, &self.options)
        } else {
            T::section(config).clone()
        }
    }

    /// How this instance is dressed and how big what it opens is: `[modules.<id>]` with its own options over it.
    pub fn presentation(&self, config: &Config) -> ModuleOverride {
        config.presentation(&self.module, &self.options)
    }
}

/// State one instance keeps across every build of it, keyed by the instance so it outlives what a rebuild drops, and forgotten once the layout stops placing that instance ([`forget`]). A reading of the system every instance shares is a service instead.
pub struct InstanceStore<T: 'static> {
    init: fn() -> T,
    slots: Mutex<BTreeMap<InstanceId, Slot<T>>>,
    listed: Once,
}

struct Slot<T> {
    value: T,
    subscribers: Vec<EventSender<T>>,
}

/// What [`forget`] reaches every store through, whatever it holds.
trait Forgets: Sync {
    fn forget(&self, instance: &InstanceId);
}

/// Every [`InstanceStore`] that has kept a value, listed by the store itself on first use, so forgetting an instance needs no list of stores kept by hand.
static STORES: Mutex<Vec<&'static dyn Forgets>> = Mutex::new(Vec::new());

/// Drops what every [`InstanceStore`] keeps for `instances`: the layout no longer places them, and an instance added later under the same id starts from its module's defaults rather than from a removed one's state (F-3.4).
pub fn forget(instances: &[InstanceId]) {
    let stores: Vec<&'static dyn Forgets> = STORES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    for store in stores {
        for instance in instances {
            store.forget(instance);
        }
    }
}

impl<T: Clone + Send + 'static> Forgets for InstanceStore<T> {
    fn forget(&self, instance: &InstanceId) {
        let gone = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(instance);
        drop(gone);
    }
}

impl<T: Clone + Send + 'static> InstanceStore<T> {
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            init,
            slots: Mutex::new(BTreeMap::new()),
            listed: Once::new(),
        }
    }

    fn with_slot<R>(&'static self, instance: &InstanceId, f: impl FnOnce(&mut Slot<T>) -> R) -> R {
        self.listed.call_once(|| {
            STORES
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(self);
        });
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let slot = slots.entry(instance.clone()).or_insert_with(|| Slot {
            value: (self.init)(),
            subscribers: Vec::new(),
        });
        f(slot)
    }

    pub fn get(&'static self, instance: &InstanceId) -> T {
        self.with_slot(instance, |slot| slot.value.clone())
    }

    /// Applies `change` to `instance`'s value and sends the result to that instance's subscribers. `change` runs under the store's lock, so it must not reach back into this store.
    pub fn update(&'static self, instance: &InstanceId, change: impl FnOnce(&mut T)) -> T {
        self.with_slot(instance, |slot| {
            change(&mut slot.value);
            let value = slot.value.clone();
            slot.subscribers.retain(|tx| tx.send(value.clone()));
            value
        })
    }

    pub fn set(&'static self, instance: &InstanceId, value: T) {
        self.update(instance, |current| *current = value);
    }

    /// Registers `tx` for changes to `instance`'s value, sending the current one immediately so a surface starts in sync.
    pub fn subscribe(&'static self, instance: &InstanceId, tx: EventSender<T>) {
        self.with_slot(instance, |slot| {
            if tx.send(slot.value.clone()) {
                slot.subscribers.push(tx);
            }
        });
    }
}

/// A widget's size, stepped by the desktop's corner handle rather than dragged to any size, so every widget of one size shares a footprint and a grid of them lines up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetSize {
    S,
    M,
    L,
}

/// The side of one desktop grid cell and the gap between two cells, in px, which is what turns a [`Footprint`] into a box.
pub const GRID_CELL: f32 = 80.0;
pub const GRID_GAP: f32 = 16.0;

/// The cells a widget covers on the desktop grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint {
    pub columns: u16,
    pub rows: u16,
}

impl Footprint {
    /// The box the footprint covers on a grid of `cell`-sized cells `gap` apart: every cell it spans and the gaps between them.
    pub fn extent(self, cell: f32, gap: f32) -> Size {
        let span = |cells: u16| {
            let cells = f32::from(cells.max(1));
            cells * cell + (cells - 1.0) * gap
        };
        Size {
            width: span(self.columns),
            height: span(self.rows),
        }
    }
}

impl WidgetSize {
    pub const ALL: [WidgetSize; 3] = [WidgetSize::S, WidgetSize::M, WidgetSize::L];

    /// The home screen's three shapes: a square, a strip two squares wide, and a square of four.
    pub const fn footprint(self) -> Footprint {
        match self {
            WidgetSize::S => Footprint {
                columns: 2,
                rows: 2,
            },
            WidgetSize::M => Footprint {
                columns: 4,
                rows: 2,
            },
            WidgetSize::L => Footprint {
                columns: 4,
                rows: 4,
            },
        }
    }

    pub fn extent(self) -> Size {
        self.footprint().extent(GRID_CELL, GRID_GAP)
    }
}

/// Who can read what an instance draws: the signed-in user, or anyone in front of the screen — the lock layer, where a reading draws only the fields its sources declare [`Privacy::Public`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Audience {
    #[default]
    Owner,
    Anyone,
}

/// One module seen at a given size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Representation {
    Chip,
    Widget(WidgetSize),
    Card,
    Panel,
    Popout,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug)]
pub struct Host {
    pub instance: InstanceId,
    module: Arc<str>,
    /// The instance's own options, read through [`Host::options`] and [`Host::presentation`].
    options: Arc<toml::Table>,
    pub representation: Representation,
    /// The box the representation is given. A chip has no length of its own along its bar, so that side is `f32::INFINITY`.
    pub extent: Size,
    /// The edge a chip's bar hangs off; `None` for a representation that runs along no bar.
    pub axis: Option<Edge>,
    pub shape: ResolvedShape,
    pub accent: Color,
    pub foreground: Color,
    /// The monitor the instance is on; `None` is the compositor's active output.
    pub output: Option<String>,
    pub audience: Audience,
    /// The module defaults [`Host::options`] lays the instance's own options over, and every behaviour key.
    config: Arc<Config>,
}

impl Host {
    /// A chip on a bar `thickness` across along `edge`, shaped by the theme: what a chip is when nothing but the edge and the thickness is known about the bar it is on.
    pub fn chip(
        instance: Instance,
        config: Arc<Config>,
        edge: Edge,
        thickness: f32,
        accent: Color,
        foreground: Color,
        output: Option<String>,
    ) -> Self {
        let extent = if edge.is_vertical() {
            Size {
                width: thickness,
                height: f32::INFINITY,
            }
        } else {
            Size {
                width: f32::INFINITY,
                height: thickness,
            }
        };
        Self {
            instance: instance.id,
            module: instance.module,
            options: instance.options,
            representation: Representation::Chip,
            extent,
            axis: Some(edge),
            shape: config.shape_from(None, None, None, None),
            accent,
            foreground,
            output,
            audience: Audience::Owner,
            config,
        }
    }

    /// A representation placed inside an area of the layout, with its box and its shape already decided.
    ///
    /// An area carries its own box and shape, and several areas can share an edge, so the caller passes what it resolved. `config` stays, because it is what `host.options()` and every behaviour key are read from.
    #[allow(clippy::too_many_arguments)]
    pub fn placed(
        instance: Instance,
        config: Arc<Config>,
        representation: Representation,
        extent: Size,
        axis: Option<Edge>,
        shape: ResolvedShape,
        accent: Color,
        foreground: Color,
        output: Option<String>,
    ) -> Self {
        Self {
            instance: instance.id,
            module: instance.module,
            options: instance.options,
            representation,
            extent,
            axis,
            shape,
            accent,
            foreground,
            output,
            audience: Audience::Owner,
            config,
        }
    }

    pub fn in_chrome(
        instance: Instance,
        representation: Representation,
        chrome: &Chrome,
        extent: Size,
    ) -> Self {
        let theme = chrome.config.resolve_theme();
        Self {
            instance: instance.id,
            module: instance.module,
            options: instance.options,
            representation,
            extent,
            axis: None,
            shape: chrome.shape,
            accent: theme.accent,
            foreground: theme.text,
            output: chrome.output.clone(),
            audience: Audience::Owner,
            config: Arc::clone(&chrome.config),
        }
    }

    /// `module`'s `representation`, `extent` across, placed inside what this host builds — a card on the dashboard's page. No instance speaks for it, so it reads its module's defaults.
    pub fn inner(&self, module: &str, representation: Representation, extent: Size) -> Self {
        let inner = Instance::of_module(module);
        Self {
            instance: inner.id,
            module: inner.module,
            options: inner.options,
            representation,
            extent,
            ..self.clone()
        }
    }

    /// The module this host builds.
    pub fn module(&self) -> &str {
        &self.module
    }

    /// The instance this host builds, for whatever it opens to speak for the same one.
    pub fn instance(&self) -> Instance {
        Instance {
            id: self.instance.clone(),
            module: Arc::clone(&self.module),
            options: Arc::clone(&self.options),
        }
    }

    /// The size a widget is being built at; `None` for any other representation.
    pub fn widget_size(&self) -> Option<WidgetSize> {
        match self.representation {
            Representation::Widget(size) => Some(size),
            _ => None,
        }
    }

    pub fn is_small(&self) -> bool {
        self.widget_size() == Some(WidgetSize::S)
    }

    pub fn shown_to(mut self, audience: Audience) -> Self {
        self.audience = audience;
        self
    }

    /// Whether this build may draw `field`: any field for the owner, and for anyone else only one the field itself says they may see.
    pub fn may_show(&self, field: &FieldDef) -> bool {
        self.audience == Audience::Owner || field.privacy.allows_anyone(&self.config.lock)
    }

    /// `value` when this build may draw `field`, else the field's typed empty value. The one path a reading takes to a private field, so what it draws for [`Audience::Anyone`] cannot hold one.
    pub fn reveal<T: Default>(&self, field: &FieldDef, value: T) -> T {
        if self.may_show(field) {
            value
        } else {
            T::default()
        }
    }

    /// The module's settings, by the section type that holds them, with the instance's own options over its module's (TA-2).
    pub fn options<T: ModuleOptions>(&self) -> T {
        self.instance().options(&self.config)
    }

    /// How the instance is dressed and how big what it opens is.
    pub fn presentation(&self) -> ModuleOverride {
        self.instance().presentation(&self.config)
    }

    /// The whole config this build resolved, for a module whose content spans other modules' sections — the dashboard's pages, a panel's language.
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    /// The extent across the axis: a bar's thickness for a chip, the shorter side for anything else.
    pub fn thickness(&self) -> f32 {
        match self.axis {
            Some(edge) if edge.is_vertical() => self.extent.width,
            Some(_) => self.extent.height,
            None => self.extent.width.min(self.extent.height),
        }
    }

    pub fn is_vertical(&self) -> bool {
        self.axis.is_some_and(Edge::is_vertical)
    }

    /// About three quarters of the thickness, so a glyph fills most of a square chip and scales with its bar.
    pub fn icon_size(&self) -> f32 {
        (self.thickness() * 0.75).round().clamp(8.0, 64.0)
    }

    /// Padding that makes a square chip exactly as wide as it is thick: an icon of ≈0.75 of the thickness plus two of these.
    pub fn inset(&self) -> f32 {
        (self.thickness() * 0.125).round().max(1.0)
    }

    /// The corner radius of a chip in this area, so a module's inner elements round like the chips beside them.
    pub fn corner_radius(&self) -> f32 {
        self.shape.chip_radius()
    }

    /// Builds under a scope of its own that provides this host, so an `.rsx` entrypoint built inside reads it with [`Host::current`] and its handlers can read it later.
    pub fn build<R>(&self, build: impl FnOnce(&Host) -> R) -> R {
        telar::Scope::with(|| {
            self.provide();
            build(self)
        })
    }

    /// Makes this host the one [`Host::current`] answers under the current owner.
    pub fn provide(&self) {
        telar::set_context(self.clone());
    }

    /// The host the enclosing build provided. An error rather than a default: a module built outside any host would size itself against a bar nobody chose.
    pub fn current() -> Result<Host, LayoutError> {
        telar::context::<Host>()
            .ok_or_else(|| LayoutError::Engine("a module was built outside any host".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::Privacy;

    fn chip(edge: Edge, thickness: u32) -> Host {
        let config: Config = toml::from_str("[theme]\nspacing=8\nradius=12\n").unwrap();
        Host::chip(
            Instance::of_module("clock"),
            Arc::new(config),
            edge,
            thickness as f32,
            Color::TRANSPARENT,
            Color::TRANSPARENT,
            None,
        )
    }

    #[test]
    fn a_chip_sizes_itself_from_the_thickness_across_its_bar() {
        for edge in Edge::ALL {
            let host = chip(edge, 34);
            assert_eq!(host.thickness(), 34.0, "{edge:?}");
            assert_eq!(host.icon_size(), 26.0, "{edge:?}");
            assert_eq!(host.inset(), 4.0, "{edge:?}");
            assert_eq!(host.is_vertical(), edge.is_vertical());
        }
        assert_eq!(
            chip(Edge::Top, 4).icon_size(),
            8.0,
            "never below a legible glyph"
        );
        assert_eq!(chip(Edge::Top, 4).inset(), 1.0);
        assert_eq!(chip(Edge::Left, 34).corner_radius(), 8.0, "12 - 8/2");
    }

    #[test]
    fn a_module_reads_its_options_from_the_config_its_host_resolved() {
        let mut config = Config::default();
        config.clock.show_date = !config.clock.show_date;
        let expected = config.clock.show_date;
        let host = Host::chip(
            Instance::of_module("clock"),
            Arc::new(config),
            Edge::Top,
            34.0,
            Color::TRANSPARENT,
            Color::TRANSPARENT,
            None,
        );
        assert_eq!(host.options::<config::ClockConfig>().show_date, expected);
    }

    fn unbuilt(_: &Host) -> crate::descriptor::Built {
        Err(LayoutError::Engine("never built".to_string()))
    }

    const CLOCK: &[crate::descriptor::ModuleDescriptor] = &[crate::descriptor::ModuleDescriptor {
        id: "clock",
        name: "Clock",
        icon: "clock",
        options: &[crate::descriptor::OptionsType::of::<config::ClockConfig>()],
        representations: crate::descriptor::Representations {
            chip: Some(crate::descriptor::ChipDef::new(
                unbuilt,
                crate::descriptor::Input::ReadOnly,
            )),
            ..crate::descriptor::Representations::NONE
        },
        actions: &[],
        sources: &[],
    }];

    fn placed(options: &str) -> Host {
        crate::descriptor::install(CLOCK);
        let config: Config = toml::from_str(
            "[clock]\nshow_date = true\ndate_format = \"%d\"\n[temperature]\nunit = \"fahrenheit\"\n[modules.clock]\naccent = \"green\"\ndrawer_width = 360\n",
        )
        .unwrap();
        Host::chip(
            Instance::new(
                InstanceId::new("clock-2"),
                "clock",
                toml::from_str(options).unwrap(),
            ),
            Arc::new(config),
            Edge::Top,
            34.0,
            Color::TRANSPARENT,
            Color::TRANSPARENT,
            None,
        )
    }

    /// TA-2's cascade: what an instance sets wins, what it leaves unset is its module's, and it reaches only its own module's section.
    #[test]
    fn an_instance_s_options_win_over_its_module_s_and_nothing_else_s() {
        let host = placed(
            "date_format = \"%A\"\nunit = \"celsius\"\naccent = \"red\"\npopout_width = 300\n",
        );
        let clock = host.options::<config::ClockConfig>();
        assert_eq!(clock.date_format, "%A", "the instance's own value wins");
        assert!(
            clock.show_date,
            "a key it does not set falls back to [clock]"
        );
        assert_eq!(
            host.options::<config::TemperatureConfig>().unit,
            config::TemperatureUnit::Fahrenheit,
            "another module's section is not the clock's to override"
        );
        let presented = host.presentation();
        assert_eq!(presented.accent.as_deref(), Some("red"));
        assert_eq!(presented.popout_width, 300.0);
        assert_eq!(
            presented.drawer_width, 360.0,
            "and [modules.clock] fills in the rest"
        );

        let bare = placed("");
        assert_eq!(bare.options::<config::ClockConfig>().date_format, "%d");
        assert_eq!(bare.presentation().accent.as_deref(), Some("green"));
        assert_eq!(bare.module(), "clock");
        assert_eq!(bare.instance().id, InstanceId::new("clock-2"));
    }

    #[test]
    fn a_build_under_a_host_reads_it_and_one_outside_does_not() {
        let _scope = telar::owner_scope();
        assert!(Host::current().is_err());
        let seen = chip(Edge::Right, 40).build(|_| Host::current());
        assert_eq!(seen.expect("provided").axis, Some(Edge::Right));
    }

    #[test]
    fn each_size_steps_up_to_a_footprint_that_holds_the_one_before() {
        let cells = |size: WidgetSize| {
            let footprint = size.footprint();
            (footprint.columns, footprint.rows)
        };
        assert_eq!(cells(WidgetSize::S), (2, 2));
        assert_eq!(cells(WidgetSize::M), (4, 2));
        assert_eq!(cells(WidgetSize::L), (4, 4));

        let small = WidgetSize::S.extent();
        assert_eq!(small.width, 2.0 * GRID_CELL + GRID_GAP);
        assert_eq!(small.width, small.height);
        let medium = WidgetSize::M.extent();
        assert_eq!(
            medium.width,
            2.0 * small.width + GRID_GAP,
            "two smalls and the gap between them"
        );
        assert_eq!(WidgetSize::L.extent().height, medium.width);
    }

    #[test]
    fn a_private_field_reads_as_empty_for_anyone_but_the_owner() {
        let secret = FieldDef {
            name: "summary",
            privacy: Privacy::Private,
        };
        let count = FieldDef {
            name: "count",
            privacy: Privacy::Public,
        };
        let owner = chip(Edge::Top, 32);
        assert_eq!(
            owner.audience,
            Audience::Owner,
            "a build is the user's unless told"
        );
        assert_eq!(owner.reveal(&secret, "hi".to_string()), "hi");

        let anyone = owner.shown_to(Audience::Anyone);
        assert_eq!(anyone.reveal(&secret, "hi".to_string()), "");
        assert!(!anyone.may_show(&secret));
        assert_eq!(anyone.reveal(&count, 3), 3);
    }

    /// A field whose privacy is a question about `[lock]` is answered from the config the build is under, so the same reading draws one thing for a user who asked for the detail and another for a user who did not.
    #[test]
    fn a_field_the_lock_config_decides_follows_that_config() {
        let asked = FieldDef {
            name: "apps",
            privacy: Privacy::OnLock(|lock| {
                lock.notification_detail == config::NotificationDetail::Apps
            }),
        };
        let anyone = chip(Edge::Top, 32).shown_to(Audience::Anyone);
        assert_eq!(
            anyone.reveal(&asked, "Bank".to_string()),
            "",
            "the count alone is the default"
        );

        let mut told = anyone.clone();
        let mut config = (**told.config()).clone();
        config.lock.notification_detail = config::NotificationDetail::Apps;
        told.config = Arc::new(config);
        assert_eq!(told.reveal(&asked, "Bank".to_string()), "Bank");
        assert_eq!(
            told.shown_to(Audience::Owner)
                .reveal(&asked, "Bank".to_string()),
            "Bank",
            "and the signed-in user sees it either way"
        );
    }

    static COUNTS: InstanceStore<u32> = InstanceStore::new(|| 0);

    #[test]
    fn each_instance_keeps_its_own_value_and_hears_only_about_its_own() {
        let (a, b) = (InstanceId::new("store-a"), InstanceId::new("store-b"));
        let (tx, heard) = platform_wayland::detached();
        COUNTS.subscribe(&b, tx);
        assert_eq!(heard.try_recv(), Some(0), "a subscriber starts in sync");

        COUNTS.set(&a, 3);
        assert_eq!(COUNTS.get(&a), 3);
        assert_eq!(COUNTS.get(&b), 0, "b starts from init, not from a");
        assert_eq!(heard.try_recv(), None, "b hears nothing about a");

        assert_eq!(COUNTS.update(&b, |n| *n += 1), 1);
        assert_eq!(heard.try_recv(), Some(1));
        assert_eq!(COUNTS.get(&a), 3);
    }

    /// A rebuild disposes everything the old tree owned and resets the layout runtime; the value is found again by the instance's id alone.
    #[test]
    fn an_instance_keeps_its_value_across_a_rebuild() {
        let built = telar::owner_scope();
        let owner = built.id();
        COUNTS.set(&InstanceId::new("store-rebuilt"), 7);
        drop(built);
        telar::dispose_owner(owner);
        telar::reset_layout_runtime();

        let _rebuilt = telar::owner_scope();
        assert_eq!(COUNTS.get(&InstanceId::new("store-rebuilt")), 7);
    }

    static LABELS: InstanceStore<String> = InstanceStore::new(String::new);

    /// F-3.4: an instance the layout stopped placing leaves nothing behind in any store, so one added later under its id starts fresh — and an instance still placed keeps what it had.
    #[test]
    fn a_forgotten_instance_starts_again_from_init_in_every_store() {
        let (gone, kept) = (InstanceId::new("store-gone"), InstanceId::new("store-kept"));
        COUNTS.set(&gone, 4);
        LABELS.set(&gone, "left".to_string());
        COUNTS.set(&kept, 9);

        forget(std::slice::from_ref(&gone));

        assert_eq!(COUNTS.get(&gone), 0);
        assert_eq!(LABELS.get(&gone), "");
        assert_eq!(COUNTS.get(&kept), 9);
    }

    #[test]
    fn a_subscriber_that_went_away_stops_being_sent_to() {
        let instance = InstanceId::new("store-departed");
        let (tx, heard) = platform_wayland::detached();
        COUNTS.subscribe(&instance, tx);
        drop(heard);
        COUNTS.set(&instance, 1);
        let live = COUNTS
            .slots
            .lock()
            .unwrap()
            .get(&instance)
            .map_or(0, |slot| slot.subscribers.len());
        assert_eq!(live, 0);
    }
}
