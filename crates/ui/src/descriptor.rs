//! What a module is, declared once: its id, the representations it can be shown as, the options it reads, the verbs it answers and the readings it exposes; the table is installed at startup so a surface can resolve a module id without naming a module.

use std::cell::Cell;

use platform_wayland::KeyboardMode;
use telar::{
    AvailableSpace, Children, Container, ErrorBoundary, LayoutError, LayoutItem, LayoutStyle, Rect,
    SizeDimension, compute_layout, interactive_rects,
};

use config::ModuleOptions;

use crate::card::{Card, Density};
use crate::host::{Host, Representation, WidgetSize};
use crate::placeholder;

pub type Built = Result<Box<dyn LayoutItem>, LayoutError>;

pub type Build = fn(&Host) -> Built;

/// Whether a representation's own build registers a press, drag, wheel or hover target anywhere in its tree. What a bar wires around a chip belongs to the placement, not the representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    ReadOnly,
    Interactive,
}

/// The config section a module's options live in, named by its type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionsType {
    pub section: &'static str,
}

impl OptionsType {
    pub const fn of<T: ModuleOptions>() -> Self {
        Self {
            section: T::SECTION,
        }
    }
}

/// A verb the module answers, as the IPC line that performs it, so `--list` and anything that offers the module's actions enumerate the same commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionDef {
    pub id: &'static str,
    pub command: &'static str,
}

/// Whether a reading may be shown where anyone can read it — the lock screen — or only to the signed-in user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Privacy {
    Public,
    Private,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldDef {
    pub name: &'static str,
    pub privacy: Privacy,
}

/// A typed reading the module exposes. Declared only: nothing produces one yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceDef {
    pub id: &'static str,
    pub fields: &'static [FieldDef],
}

/// How a bar places a chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipFrame {
    /// Inside the chip shell, which pads it, paints its hover and takes its presses.
    Shell,
    /// Bare: the module lays itself out and takes its own presses.
    SelfManaged,
    /// Room rather than a chip, placed bare on no surface even on a chip bar.
    Filler,
}

#[derive(Clone, Copy)]
pub struct ChipDef {
    pub build: Build,
    pub input: Input,
    pub frame: ChipFrame,
    /// A square chip that scales with the bar instead of a content-width pill.
    pub square: bool,
    /// What pressing the chip runs instead of toggling the module's panel.
    pub press: Option<fn()>,
    /// What the wheel does over the chip, as `(dx, dy)` in pixels, given the host the chip was built under so it acts on the same options it draws with.
    pub scroll: Option<fn(&Host, f32, f32)>,
    /// Gives up width when its zone runs short, eliding its label, instead of holding its content width.
    pub elastic: bool,
}

impl ChipDef {
    pub const fn new(build: Build, input: Input) -> Self {
        Self {
            build,
            input,
            frame: ChipFrame::Shell,
            square: false,
            press: None,
            scroll: None,
            elastic: false,
        }
    }

    pub const fn square(mut self) -> Self {
        self.square = true;
        self
    }

    pub const fn on_press(mut self, press: fn()) -> Self {
        self.press = Some(press);
        self
    }

    pub const fn on_scroll(mut self, scroll: fn(&Host, f32, f32)) -> Self {
        self.scroll = Some(scroll);
        self
    }

    pub const fn self_managed(mut self) -> Self {
        self.frame = ChipFrame::SelfManaged;
        self
    }

    pub const fn filler(mut self) -> Self {
        self.frame = ChipFrame::Filler;
        self
    }

    pub const fn elastic(mut self) -> Self {
        self.elastic = true;
        self
    }

    pub fn is_bare(&self) -> bool {
        self.frame != ChipFrame::Shell
    }
}

/// A module at a size of its own on a grid, built for `Host.representation = Widget(size)` into the size's footprint. A `ReadOnly` one is also a reading: what the lock layer may place.
#[derive(Clone, Copy)]
pub struct WidgetDef {
    pub sizes: &'static [WidgetSize],
    pub build: Build,
    pub input: Input,
}

/// A card on a dashboard page or in a hover popout. It yields a [`Card`] rather than a tree so whoever places it picks the density and dresses the box.
#[derive(Clone, Copy)]
pub struct CardDef {
    pub build: fn(&Host) -> Card,
    pub input: Input,
}

#[derive(Clone, Copy)]
pub struct PanelDef {
    pub build: Build,
    pub input: Input,
    /// Whether opening the panel takes the keyboard. Only a panel that hosts editable text or is navigable with the arrow keys asks: a layer surface granted keyboard focus takes it from the focused window, and the compositor re-focuses that window when the panel closes, which moves the viewport under a focus-following layout.
    pub keyboard: KeyboardMode,
    /// What the panel keeps only while it is open, dropped when the user closes it. A reload leaves open panels alone, so this fires exactly on "the user is done with it".
    pub on_close: Option<fn()>,
}

impl PanelDef {
    pub const fn new(build: Build, input: Input) -> Self {
        Self {
            build,
            input,
            keyboard: KeyboardMode::None,
            on_close: None,
        }
    }

    pub const fn keyboard(mut self, keyboard: KeyboardMode) -> Self {
        self.keyboard = keyboard;
        self
    }

    pub const fn on_close(mut self, forget: fn()) -> Self {
        self.on_close = Some(forget);
        self
    }
}

#[derive(Clone, Copy)]
pub struct Representations {
    pub chip: Option<ChipDef>,
    pub widget: Option<WidgetDef>,
    pub card: Option<CardDef>,
    pub panel: Option<PanelDef>,
    pub popout: Option<CardDef>,
}

impl Representations {
    pub const NONE: Self = Self {
        chip: None,
        widget: None,
        card: None,
        panel: None,
        popout: None,
    };
}

#[derive(Clone, Copy)]
pub struct ModuleDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    /// The glyph a palette or a popover shows the module by.
    pub icon: &'static str,
    /// The sections it reads, its own first; empty for a module with nothing to configure. More than one when what it draws is also configured elsewhere — the desktop clock face by `[widgets]`, a temperature by `[temperature] unit`.
    pub options: &'static [OptionsType],
    pub representations: Representations,
    pub actions: &'static [ActionDef],
    pub sources: &'static [SourceDef],
}

impl ModuleDescriptor {
    /// Every representation this module declares, each widget size its own.
    pub fn declared(&self) -> Vec<Representation> {
        let r = &self.representations;
        let mut declared = Vec::new();
        if r.chip.is_some() {
            declared.push(Representation::Chip);
        }
        if let Some(widget) = &r.widget {
            declared.extend(
                widget
                    .sizes
                    .iter()
                    .map(|size| Representation::Widget(*size)),
            );
        }
        if r.card.is_some() {
            declared.push(Representation::Card);
        }
        if r.panel.is_some() {
            declared.push(Representation::Panel);
        }
        if r.popout.is_some() {
            declared.push(Representation::Popout);
        }
        declared
    }

    pub fn input(&self, representation: Representation) -> Option<Input> {
        let r = &self.representations;
        match representation {
            Representation::Chip => r.chip.map(|chip| chip.input),
            Representation::Widget(size) => r
                .widget
                .filter(|widget| widget.sizes.contains(&size))
                .map(|widget| widget.input),
            Representation::Card => r.card.map(|card| card.input),
            Representation::Panel => r.panel.map(|panel| panel.input),
            Representation::Popout => r.popout.map(|popout| popout.input),
        }
    }

    /// Builds `host.representation` under `host`, or `None` when the module does not declare it. A popout is boxed like the panels of the bar it opens from, `host.extent` wide.
    pub fn build(&self, host: &Host) -> Option<Built> {
        let r = &self.representations;
        let build: Build = match host.representation {
            Representation::Chip => r.chip?.build,
            Representation::Widget(size) => r.widget.filter(|w| w.sizes.contains(&size))?.build,
            Representation::Panel => r.panel?.build,
            Representation::Card => {
                let card = r.card?;
                return Some(host.build(|host| (card.build)(host).build(Density::Page)));
            }
            Representation::Popout => {
                let popout = r.popout?;
                return Some(host.build(|host| {
                    (popout.build)(host)
                        .fill(host.config().panel_fill())
                        .radius(host.shape.radius)
                        .width(host.extent.width)
                        .build(Density::Compact)
                }));
            }
        };
        Some(host.build(build))
    }
}

pub fn lookup<'a>(table: &'a [ModuleDescriptor], id: &str) -> Option<&'a ModuleDescriptor> {
    table.iter().find(|descriptor| descriptor.id == id)
}

thread_local! {
    static INSTALLED: Cell<&'static [ModuleDescriptor]> = const { Cell::new(&[]) };
}

/// Publishes the module table every surface resolves against. Set once at startup.
pub fn install(table: &'static [ModuleDescriptor]) {
    INSTALLED.with(|installed| installed.set(table));
}

/// The installed table — empty when nothing was installed, which is what a test that never composed a shell sees.
pub fn installed() -> &'static [ModuleDescriptor] {
    INSTALLED.with(Cell::get)
}

pub fn find(id: &str) -> Option<&'static ModuleDescriptor> {
    lookup(installed(), id)
}

fn panel_of(module: &str) -> Option<PanelDef> {
    find(module)?.representations.panel
}

pub fn has_panel(module: &str) -> bool {
    panel_of(module).is_some()
}

pub fn has_popout(module: &str) -> bool {
    find(module).is_some_and(|descriptor| descriptor.representations.popout.is_some())
}

pub fn wants_keyboard(module: &str) -> KeyboardMode {
    panel_of(module).map_or(KeyboardMode::None, |panel| panel.keyboard)
}

/// Tells `module`'s panel the user closed it, so whatever it kept for the length of that visit is dropped.
pub fn closed(module: &str) {
    if let Some(forget) = panel_of(module).and_then(|panel| panel.on_close) {
        forget();
    }
}

/// Builds `build` in a box laid out by `style`, so that a failure — an error or a panic, during the build or in a later re-run of what it built — is logged and shows `id`'s placeholder for `host` in its place instead of failing the surface around it.
pub fn guard(
    id: &str,
    host: &Host,
    style: LayoutStyle,
    build: impl FnOnce() -> Built + 'static,
) -> Built {
    let id = id.to_string();
    let host = host.clone();
    let boundary = ErrorBoundary::with_style(style, build, move |failure| {
        tracing::error!("'{id}' as {:?}: {failure}", host.representation);
        let theme = host.config().resolve_theme();
        placeholder::placeholder(&id, Some(&failure.to_string()), &host, theme)
    })?;
    Ok(Box::new(boundary))
}

/// `id`'s `host.representation` from the installed table, guarded by [`guard`]: an id no module answers to and a representation it does not declare fail the same way a build does.
pub fn place(id: &str, host: &Host, style: LayoutStyle) -> Built {
    let descriptor = find(id).copied();
    let module = id.to_string();
    let built_host = host.clone();
    guard(id, host, style, move || {
        let descriptor = descriptor
            .ok_or_else(|| LayoutError::Engine(format!("no module answers to '{module}'")))?;
        descriptor.build(&built_host).unwrap_or_else(|| {
            Err(LayoutError::Engine(format!(
                "'{module}' declares no {:?}",
                built_host.representation
            )))
        })
    })
}

/// The panel `host` was opened for, placed across the width it is given.
pub fn build_panel(host: &Host) -> Built {
    debug_assert_eq!(host.representation, Representation::Panel);
    place(
        host.instance.module(),
        host,
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0)),
    )
}

/// A module's panel, as an element for a surface that places it in markup.
#[derive(telar::Props)]
pub struct PanelProps {
    pub host: Host,
}

pub fn panel(props: PanelProps, _children: Children) -> Built {
    build_panel(&props.host)
}

/// Every rect a tree registered a press, drag, wheel or hover target on once laid out in a `width` × `height` box: the observable behind [`Input::ReadOnly`], read from what the compositor's input region is built from. Everything registered on the current layout runtime counts, so the caller resets it first.
pub fn input_targets(
    item: Box<dyn LayoutItem>,
    width: f32,
    height: f32,
) -> Result<Vec<Rect>, LayoutError> {
    let page = Container::new(
        LayoutStyle::new().flex_column().width(width).height(height),
        vec![item],
    )?;
    compute_layout(
        page.layout_node(),
        AvailableSpace::Definite(width),
        AvailableSpace::Definite(height),
    )?;
    let targets = interactive_rects();
    drop(page);
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::theme::NordTheme;
    use std::sync::Arc;
    use telar::{
        ComponentList, DrawCommand, Paint, RectStyle, StyledContainer, reset_layout_runtime,
        set_theme,
    };

    use crate::host::{InstanceId, Size};

    fn reading(_host: &Host) -> Built {
        Ok(Box::new(StyledContainer::new(
            LayoutStyle::new().width(40.0).height(20.0),
            |_| RectStyle::filled(telar::Color::TRANSPARENT, 0.0),
            vec![],
        )?))
    }

    fn pressable(_host: &Host) -> Built {
        Ok(Box::new(
            StyledContainer::new(
                LayoutStyle::new().width(40.0).height(20.0),
                |_| RectStyle::filled(telar::Color::TRANSPARENT, 0.0),
                vec![],
            )?
            .on_press(|| {}),
        ))
    }

    fn scrollable(_host: &Host) -> Built {
        Ok(Box::new(
            StyledContainer::new(
                LayoutStyle::new().width(40.0).height(20.0),
                |_| RectStyle::filled(telar::Color::TRANSPARENT, 0.0),
                vec![],
            )?
            .on_scroll(|_, _| {}),
        ))
    }

    fn hoverable(_host: &Host) -> Built {
        Ok(Box::new(
            StyledContainer::new(
                LayoutStyle::new().width(40.0).height(20.0),
                |_| RectStyle::filled(telar::Color::TRANSPARENT, 0.0),
                vec![],
            )?
            .on_hover(|_| {}),
        ))
    }

    fn draggable(_host: &Host) -> Built {
        Ok(Box::new(
            StyledContainer::new(
                LayoutStyle::new().width(40.0).height(20.0),
                |_| RectStyle::filled(telar::Color::TRANSPARENT, 0.0),
                vec![],
            )?
            .on_drag(|_, _| {}),
        ))
    }

    fn chip_host() -> Host {
        Host::chip(
            InstanceId::of_module("probe"),
            Arc::new(config::Config::default()),
            config::Edge::Top,
            telar::Color::TRANSPARENT,
            telar::Color::TRANSPARENT,
            None,
        )
    }

    fn targets_of(build: Build) -> Vec<Rect> {
        reset_layout_runtime();
        let _scope = telar::owner_scope();
        let item = chip_host().build(build).expect("the probe builds");
        input_targets(item, 200.0, 40.0).expect("the probe lays out")
    }

    /// The check a `ReadOnly` declaration is held to has to be able to fail.
    #[test]
    fn every_kind_of_input_target_is_observed_and_a_reading_registers_none() {
        assert!(targets_of(reading).is_empty(), "a bare box answers nothing");
        for (kind, build) in [
            ("press", pressable as Build),
            ("drag", draggable),
            ("scroll", scrollable),
            ("hover", hoverable),
        ] {
            assert_eq!(
                targets_of(build).len(),
                1,
                "a {kind} target went unobserved, so a ReadOnly representation carrying one would pass"
            );
        }
    }

    fn probe(representations: Representations) -> ModuleDescriptor {
        ModuleDescriptor {
            id: "probe",
            name: "Probe",
            icon: "circle",
            options: &[],
            representations,
            actions: &[],
            sources: &[],
        }
    }

    #[test]
    fn a_declared_representation_is_built_and_an_undeclared_one_is_not() {
        let descriptor = probe(Representations {
            chip: Some(ChipDef::new(reading, Input::ReadOnly)),
            ..Representations::NONE
        });
        assert_eq!(descriptor.declared(), vec![Representation::Chip]);
        assert_eq!(
            descriptor.input(Representation::Chip),
            Some(Input::ReadOnly)
        );
        assert_eq!(descriptor.input(Representation::Panel), None);
        reset_layout_runtime();
        let _scope = telar::owner_scope();
        let host = chip_host();
        assert!(descriptor.build(&host).is_some_and(|built| built.is_ok()));
        let mut panel_host = host.clone();
        panel_host.representation = Representation::Panel;
        assert!(descriptor.build(&panel_host).is_none());
    }

    fn panics(_host: &Host) -> Built {
        panic!("a module that panics while it builds")
    }

    fn fails(_host: &Host) -> Built {
        Err(LayoutError::Engine("a module that fails to build".into()))
    }

    fn panicking_card(_host: &Host) -> Card {
        panic!("a module whose card panics")
    }

    fn failing_card(_host: &Host) -> Card {
        Card::bare().child(|| Err(LayoutError::Engine("a card that fails to build".into())))
    }

    static BROKEN: &[ModuleDescriptor] = &[
        ModuleDescriptor {
            id: "panics",
            name: "Panics",
            icon: "circle",
            options: &[],
            representations: Representations {
                chip: Some(ChipDef::new(panics, Input::ReadOnly)),
                widget: Some(WidgetDef {
                    sizes: &WidgetSize::ALL,
                    build: panics,
                    input: Input::ReadOnly,
                }),
                card: Some(CardDef {
                    build: panicking_card,
                    input: Input::ReadOnly,
                }),
                panel: Some(PanelDef::new(panics, Input::ReadOnly)),
                popout: Some(CardDef {
                    build: panicking_card,
                    input: Input::ReadOnly,
                }),
            },
            actions: &[],
            sources: &[],
        },
        ModuleDescriptor {
            id: "fails",
            name: "Fails",
            icon: "circle",
            options: &[],
            representations: Representations {
                chip: Some(ChipDef::new(fails, Input::ReadOnly)),
                widget: Some(WidgetDef {
                    sizes: &WidgetSize::ALL,
                    build: fails,
                    input: Input::ReadOnly,
                }),
                card: Some(CardDef {
                    build: failing_card,
                    input: Input::ReadOnly,
                }),
                panel: Some(PanelDef::new(fails, Input::ReadOnly)),
                popout: Some(CardDef {
                    build: failing_card,
                    input: Input::ReadOnly,
                }),
            },
            actions: &[],
            sources: &[],
        },
        ModuleDescriptor {
            id: "works",
            name: "Works",
            icon: "circle",
            options: &[],
            representations: Representations {
                panel: Some(PanelDef::new(reading, Input::ReadOnly)),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        },
    ];

    fn placeholders_drawn(tree: &ComponentList, theme: NordTheme) -> usize {
        tree.commands()
            .iter()
            .filter(|command| {
                matches!(command, DrawCommand::Rect { style, .. } if style.fill == Some(Paint::Solid(placeholder::fill(theme))))
            })
            .count()
    }

    /// Each representation of a module that panics or errs while building is placed as a placeholder, and the module built beside it on the same surface still draws.
    #[test]
    fn a_failing_representation_is_a_placeholder_and_its_surface_still_builds() {
        install(BROKEN);
        let theme = NordTheme::new();
        let config = Arc::new(config::Config::starter());
        let representations = [
            Representation::Chip,
            Representation::Widget(WidgetSize::S),
            Representation::Card,
            Representation::Panel,
            Representation::Popout,
        ];
        for broken in ["panics", "fails", "absent"] {
            for representation in representations {
                reset_layout_runtime();
                set_theme(theme);
                let scope = telar::owner_scope();
                let owner = scope.id();
                let extent = Size {
                    width: 320.0,
                    height: 200.0,
                };
                let host =
                    crate::preview::host_on(Arc::clone(&config), broken, representation, extent);
                let neighbour = crate::preview::host_on(
                    Arc::clone(&config),
                    "works",
                    Representation::Panel,
                    extent,
                );
                let failed = place(broken, &host, LayoutStyle::new()).unwrap_or_else(|e| {
                    panic!("{broken} as {representation:?} took the surface down: {e}")
                });
                let built = build_panel(&neighbour).expect("the module beside it builds");
                let surface = Container::new(
                    LayoutStyle::new().flex_column().width(extent.width),
                    vec![failed, built],
                )
                .expect("the surface builds");
                let root = surface.layout_node();
                let tree = ComponentList::new(surface);
                compute_layout(
                    root,
                    AvailableSpace::Definite(extent.width),
                    AvailableSpace::Definite(1000.0),
                )
                .expect("the surface lays out");
                assert!(
                    placeholders_drawn(&tree, theme) > 0,
                    "{broken} as {representation:?} is drawn as one placeholder"
                );
                drop(tree);
                drop(scope);
                telar::dispose_owner(owner);
            }
        }
        install(&[]);
    }
}
