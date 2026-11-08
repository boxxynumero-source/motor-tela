use std::time::Instant;
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{ColorFormat, CursorCaptureSettings, DrawBorderSettings, Settings},
};

struct Captura {
    inicio: Instant,
    ultimo: Instant,
    quadros: u32,
    primeiro: bool,
}

impl GraphicsCaptureApiHandler for Captura {
    type Flags = ();
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(_ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            inicio: Instant::now(),
            ultimo: Instant::now(),
            quadros: 0,
            primeiro: true,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        controle: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self.primeiro {
            println!("Capturando a tela: {}x{}", frame.width(), frame.height());
            self.primeiro = false;
        }
        self.quadros += 1;

        if self.ultimo.elapsed().as_secs_f32() >= 1.0 {
            println!("{} quadros por segundo", self.quadros);
            self.quadros = 0;
            self.ultimo = Instant::now();
        }

        if self.inicio.elapsed().as_secs() >= 10 {
            println!("Fim do teste de 10 segundos.");
            controle.stop();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn main() {
    let monitor = Monitor::primary().expect("Nenhum monitor encontrado");
    let config = Settings::new(
        monitor,
        CursorCaptureSettings::Default,
        DrawBorderSettings::Default,
        ColorFormat::Rgba8,
        (),
    );
    Captura::start(config).expect("A captura falhou");
}
