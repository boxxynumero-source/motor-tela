use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameType};
use openh264::formats::{RgbSliceU8, YUVBuffer};
use openh264::OpenH264API;
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{ColorFormat, CursorCaptureSettings, DrawBorderSettings, Settings},
};

const TAM_PEDACO: usize = 1200;

struct Config {
    destino: SocketAddr,
    bps: u32,
}

// O codificador so e usado nesta thread, entao e seguro marcar como Send.
struct EncSeguro(Encoder);
unsafe impl Send for EncSeguro {}

struct Transmissor {
    socket: UdpSocket,
    destino: SocketAddr,
    encoder: EncSeguro,
    rgb: Vec<u8>,
    id_quadro: u32,
    ultimo_forcado: Instant,
    ult_seg: Instant,
    quadros: u32,
    bytes: u64,
}

impl Transmissor {
    fn pedido_de_chave(&self) -> bool {
        let mut b = [0u8; 16];
        let mut pedido = false;
        while let Ok((n, _)) = self.socket.recv_from(&mut b) {
            if n > 0 && b[0] == b'K' {
                pedido = true;
            }
        }
        pedido
    }
}

impl GraphicsCaptureApiHandler for Transmissor {
    type Flags = Config;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let cfg = ctx.flags;
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        let config = EncoderConfig::new().bitrate(BitRate::from_bps(cfg.bps));
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| format!("{:?}", e))?;
        Ok(Self {
            socket,
            destino: cfg.destino,
            encoder: EncSeguro(encoder),
            rgb: Vec::new(),
            id_quadro: 0,
            ultimo_forcado: Instant::now(),
            ult_seg: Instant::now(),
            quadros: 0,
            bytes: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _controle: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let total_w = frame.width() as usize;
        let total_h = frame.height() as usize;
        let w = total_w & !1;
        let h = total_h & !1;

        let mut buffer = frame.buffer().map_err(|e| format!("{:?}", e))?;
        let dados = buffer
            .as_nopadding_buffer()
            .map_err(|e| format!("{:?}", e))?;
        if dados.len() < total_w * total_h * 4 {
            return Ok(());
        }

        self.rgb.clear();
        for y in 0..h {
            let ini = y * total_w * 4;
            for px in dados[ini..ini + w * 4].chunks_exact(4) {
                self.rgb.extend_from_slice(&px[..3]);
            }
        }

        let pedido = self.pedido_de_chave();
        if pedido || self.ultimo_forcado.elapsed() >= Duration::from_secs(2) {
            self.encoder.0.force_intra_frame();
            self.ultimo_forcado = Instant::now();
        }

        let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(&self.rgb, (w, h)));
        let stream = self
            .encoder
            .0
            .encode(&yuv)
            .map_err(|e| format!("{:?}", e))?;
        let chave = matches!(stream.frame_type(), FrameType::IDR | FrameType::I);
        let h264 = stream.to_vec();
        if h264.is_empty() {
            return Ok(());
        }

        let total = (h264.len() + TAM_PEDACO - 1) / TAM_PEDACO;
        for (i, pedaco) in h264.chunks(TAM_PEDACO).enumerate() {
            let mut pacote = Vec::with_capacity(9 + pedaco.len());
            pacote.extend_from_slice(&self.id_quadro.to_be_bytes());
            pacote.extend_from_slice(&(i as u16).to_be_bytes());
            pacote.extend_from_slice(&(total as u16).to_be_bytes());
            pacote.push(if chave { 1 } else { 0 });
            pacote.extend_from_slice(pedaco);
            let _ = self.socket.send_to(&pacote, self.destino);
        }
        self.id_quadro = self.id_quadro.wrapping_add(1);

        self.quadros += 1;
        self.bytes += h264.len() as u64;
        if self.ult_seg.elapsed() >= Duration::from_secs(1) {
            println!(
                "{} quadros/s, {:.1} Mbps reais, {}x{}",
                self.quadros,
                (self.bytes as f64 * 8.0) / 1_000_000.0,
                w,
                h
            );
            self.quadros = 0;
            self.bytes = 0;
            self.ult_seg = Instant::now();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub fn executar(ip: &str, porta: &str, mbps: &str) {
    let destino: SocketAddr = match format!("{}:{}", ip, porta).parse() {
        Ok(d) => d,
        Err(_) => {
            println!("IP ou porta invalidos. Exemplo de IP: 192.168.0.10");
            return;
        }
    };
    let mbps: f64 = mbps.replace(',', ".").parse().unwrap_or(8.0);
    let bps = (mbps * 1_000_000.0) as u32;

    let monitor = Monitor::primary().expect("Nenhum monitor encontrado");
    let config = Settings::new(
        monitor,
        CursorCaptureSettings::Default,
        DrawBorderSettings::Default,
        ColorFormat::Rgba8,
        Config { destino, bps },
    );
    println!("Transmitindo para {} a {:.1} Mbps. Feche a janela para parar.", destino, mbps);
    if let Err(e) = Transmissor::start(config) {
        println!("A captura falhou: {:?}", e);
    }
}
