// Hooks for the runtime's `input.rs` (see ../hooks.rs).
&[
    // Once per frame for each of the 4 pads: feeds native gamepads (this build
    // has no XInput and never fills the DirectInput stick table).
    ("pad_update", "rage::ioPad::Update(bool)"),
    // The game's text box (save names, the phone's cheat entry, ...): touch
    // devices show the system keyboard while it is open.
    ("text_box_open", "CTextInputBox::Open(rage::ioVirtualKeyboard::Params const&)"),
    // Its movie goes away however it closes: Close(), or its own update
    // when the text is accepted or cancelled (which does not call Close()).
    ("text_box_close", "CTextInputBox::RemoveTextInputBoxMovie()"),
]
