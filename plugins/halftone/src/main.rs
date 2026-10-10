//! CMYK Halftone as a standalone plug-in executable that speaks the external
//! contract (`render <request.json>`).

fn main() -> std::process::ExitCode {
    photo_airt_sdk::serve(&photo_airt_halftone::STYLE)
}
