//! Render the UI without a display or connected device: cargo run --example preview.
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::rc::Rc;
slint::include_modules!();
struct PreviewPlatform(Rc<MinimalSoftwareWindow>);
impl slint::platform::Platform for PreviewPlatform {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}
fn strings(values: &[&str]) -> ModelRc<SharedString> {
    Rc::new(VecModel::from(
        values
            .iter()
            .map(|v| SharedString::from(*v))
            .collect::<Vec<_>>(),
    ))
    .into()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(PreviewPlatform(window.clone())))?;
    let ui = AppWindow::new()?;
    ui.set_device_connected(true);
    ui.set_device_name("Mac mini (M1, Late 2020)".into());
    ui.set_mode_label("DFU Mode".into());
    ui.set_mode_color(slint::Color::from_rgb_u8(215, 249, 165));
    ui.set_tools_ready(true);
    ui.set_can_restore(true);
    ui.set_specs(strings(&[
        "Model    Macmini9,1",
        "Board    J274AP",
        "ECID     0x19492036c0801e",
        "CPID/BDID 0x8103 / 0x22",
        "USB      05ac:1227",
    ]));
    ui.set_firmware_model(strings(&["27.2 (26B123) · signed · 23.8 GB"]));
    ui.set_log_lines(strings(&[
        "Preview only — no device operations are performed.",
        "Device identified: Macmini9,1 · DFU mode",
    ]));
    ui.show()?;
    for (name, width, height) in [("default", 1140, 840), ("compact", 980, 780)] {
        window.set_size(slint::PhysicalSize::new(width, height));
        let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height);
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(buffer.make_mut_slice(), width as usize);
        });
        let mut bytes = format!("P6\n{} {}\n255\n", width, height).into_bytes();
        bytes.extend_from_slice(buffer.as_bytes());
        std::fs::write(format!("/tmp/pear-preview-{name}.ppm"), bytes)?;
    }
    Ok(())
}
