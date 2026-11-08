use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameType};
use openh264::formats::YUVBuffer;
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
    socket: Arc<UdpSocket>,
    destino: SocketAddr,
    bps: u32,
}

// O codificador so e usado nesta thread, entao e seguro marcar como Send.
struct EncSeguro(Encoder);
unsafe impl Send for EncSeguro {}

struct Transmissor {
    socket: Arc<UdpSocket>,
    destino: SocketAddr,
    encoder: EncSeguro,
    id_quadro: u32,
    ultimo_forcado: Instant,
    ult_seg: Instant,
    quadros: u32,
    bytes: u64,
    t_copia: f64,
    t_yuv: f64,
    t_cod: f64,
}

// Converte RGBA direto para YUV 4:2:0 (I420) com contas de numeros inteiros.
// Os numeros sao os mesmos da conversao padrao BT.601 (faixa limitada).
fn rgba_para_yuv(rgba: &[u8], largura_total: usize, w: usize, h: usize, saida: &mut [u8]) {
    let (y_plano, resto) = saida.split_at_mut(w * h);
    let (u_plano, v_plano) = resto.split_at_mut(w * h / 4);
    let meia = w / 2;
    for cy in 0..h / 2 {
        let linha0 = (cy * 2) * largura_total * 4;
        let linha1 = linha0 + largura_total * 4;
        for cx in 0..meia {
            let off = cx * 2 * 4;
            let mut soma_r = 0i32;
            let mut soma_g = 0i32;
            let mut soma_b = 0i32;
            for (dy, &linha) in [linha0, linha1].iter().enumerate() {
                for dx in 0..2usize {
                    let i = linha + off + dx * 4;
                    let r = rgba[i] as i32;
                    let g = rgba[i + 1] as i32;
                    let b = rgba[i + 2] as i32;
                    y_plano[(cy * 2 + dy) * w + cx * 2 + dx] =
                        (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
                    soma_r += r;
                    soma_g += g;
                    soma_b += b;
                }
            }
            let r = soma_r / 4;
            let g = soma_g / 4;
            let b = soma_b / 4;
            u_plano[cy * meia + cx] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            v_plano[cy * meia + cx] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
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
        cfg.socket.set_nonblocking(true)?;
        let config = EncoderConfig::new().bitrate(BitRate::from_bps(cfg.bps));
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| format!("{:?}", e))?;
        Ok(Self {
            socket: cfg.socket,
            destino: cfg.destino,
            encoder: EncSeguro(encoder),
            id_quadro: 0,
            ultimo_forcado: Instant::now(),
            ult_seg: Instant::now(),
            quadros: 0,
            bytes: 0,
            t_copia: 0.0,
            t_yuv: 0.0,
            t_cod: 0.0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _controle: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let t0 = Instant::now();
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
        let t1 = Instant::now();

        let mut yuv_dados = vec![0u8; w * h * 3 / 2];
        rgba_para_yuv(&dados[..], total_w, w, h, &mut yuv_dados);
        let yuv = YUVBuffer::from_vec(yuv_dados, w, h);
        let t2 = Instant::now();

        let pedido = self.pedido_de_chave();
        if pedido || self.ultimo_forcado.elapsed() >= Duration::from_secs(2) {
            self.encoder.0.force_intra_frame();
            self.ultimo_forcado = Instant::now();
        }

        let stream = self
            .encoder
            .0
            .encode(&yuv)
            .map_err(|e| format!("{:?}", e))?;
        let chave = matches!(stream.frame_type(), FrameType::IDR | FrameType::I);
        let h264 = stream.to_vec();
        let t3 = Instant::now();
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
        self.t_copia += (t1 - t0).as_secs_f64() * 1000.0;
        self.t_yuv += (t2 - t1).as_secs_f64() * 1000.0;
        self.t_cod += (t3 - t2).as_secs_f64() * 1000.0;
        if self.ult_seg.elapsed() >= Duration::from_secs(1) {
            let n = self.quadros.max(1) as f64;
            println!(
                "{} quadros/s, {:.1} Mbps reais, {}x{} | por quadro: copia {:.0} ms, yuv {:.0} ms, codif {:.0} ms",
                self.quadros,
                (self.bytes as f64 * 8.0) / 1_000_000.0,
                w,
                h,
                self.t_copia / n,
                self.t_yuv / n,
                self.t_cod / n
            );
            self.quadros = 0;
            self.bytes = 0;
            self.t_copia = 0.0;
            self.t_yuv = 0.0;
            self.t_cod = 0.0;
            self.ult_seg = Instant::now();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub fn executar(socket: UdpSocket, destino: SocketAddr, mbps: &str) {
    let mbps: f64 = mbps.replace(',', ".").parse().unwrap_or(8.0);
    let bps = (mbps * 1_000_000.0) as u32;

    let monitor = Monitor::primary().expect("Nenhum monitor encontrado");
    let config = Settings::new(
        monitor,
        CursorCaptureSettings::Default,
        DrawBorderSettings::Default,
        ColorFormat::Rgba8,
        Config {
            socket: Arc::new(socket),
            destino,
            bps,
        },
    );
    println!("Transmitindo para {} a {:.1} Mbps. Feche a janela para parar.", destino, mbps);
    if let Err(e) = Transmissor::start(config) {
        println!("A captura falhou: {:?}", e);
    }
}
