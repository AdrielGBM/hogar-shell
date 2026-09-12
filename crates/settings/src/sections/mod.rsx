[logic]
//! The forms. One `.rsx` component per form, plus the few this directory still keeps in Rust.
//!
//! The split is what a form's rows are: a fixed sequence of fields is a component, and a sequence the machine decides the length of — every installed application, every folder in a wallpaper library, every module in the registry — is not, because a `.rsx` view lists its children in the source. Those stay grouped by area rather than by page, so two pages over one service are one file.
