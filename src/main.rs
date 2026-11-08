use std::time::Instant;
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    encoder::{
        AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
        VideoSettingsSubType,
    },
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{ColorFormat, CursorCaptureSettings, DrawBorderSettings, Settings},
};

struct Captura {
    encoder: Option<VideoEncoder>,
    inicio: Instant,
    quadros: u32,
}

impl GraphicsCaptureApiHandler for Captura {
    type Flags = ();
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(_ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            encoder: None,
            inicio: Instant::now(),
            quadros: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        controle: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        // O codificador é criado no primeiro quadro, quando já sabemos o tamanho da tela.
        if self.encoder.is_none() {
            let largura = frame.width();
            let altura = frame.height();
            println!("Capturando a tela: {}x{}", largura, altura);
            println!("Gravando em teste-nativo.mp4 (H.264, 60 fps, 15 Mbps)...");

            let encoder = VideoEncoder::new(
                VideoSettingsBuilder::new(largura, altura)
                    .sub_type(VideoSettingsSubType::H264)
                    .frame_rate(60)
                    .bitrate(15_000_000),
                AudioSettingsBuilder::default().disabled(true),
                ContainerSettingsBuilder::default(),
                "teste-nativo.mp4",
            )?;
            self.encoder = Some(encoder);
            self.inicio = Instant::now();
        }

        if let Some(encoder) = self.encoder.as_mut() {
            encoder.send_frame(frame)?;
            self.quadros += 1;
        }

        if self.inicio.elapsed().as_secs() >= 10 {
            if let Some(encoder) = self.encoder.take() {
                encoder.finish()?;
            }
            println!("Pronto. {} quadros enviados ao codificador.", self.quadros);
            println!("Arquivo salvo: teste-nativo.mp4");
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
