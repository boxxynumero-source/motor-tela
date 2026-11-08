use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use minifb::{Key, ScaleMode, Window, WindowOptions};
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::formats::YUVSource;
use openh264::OpenH264API;

struct Parcial {
    total: usize,
    recebidos: usize,
    pedacos: Vec<Option<Vec<u8>>>,
    chave: bool,
}

// Converte o quadro decodificado (YUV 4:2:0) direto para pixels 0RGB, com contas de inteiros.
fn yuv_para_pixels(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    strides: (usize, usize, usize),
    dims: (usize, usize),
    saida: &mut [u32],
) {
    let (ys, us, vs) = strides;
    let (w, h) = dims;
    for linha in 0..h {
        let yl = &y[linha * ys..linha * ys + w];
        let ul = &u[(linha / 2) * us..(linha / 2) * us + w / 2];
        let vl = &v[(linha / 2) * vs..(linha / 2) * vs + w / 2];
        let out = &mut saida[linha * w..(linha + 1) * w];
        for x in 0..w {
            let c = 298 * (yl[x] as i32 - 16);
            let d = ul[x / 2] as i32 - 128;
            let e = vl[x / 2] as i32 - 128;
            let r = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u32;
            let g = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u32;
            let b = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u32;
            out[x] = (r << 16) | (g << 8) | b;
        }
    }
}

// `par` = endereco do outro PC quando a conexao ja foi feita pela internet.
// Com `None` (IP direto), o programa aprende o endereco do primeiro pacote que chegar.
pub fn executar(socket: UdpSocket, par: Option<SocketAddr>) {
    let socket_resp = socket.try_clone().expect("Falha ao clonar o socket");

    // Uma thread so para ler a rede, para nao perder pacotes enquanto a janela desenha.
    let (tx, rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>();
    thread::spawn(move || {
        let mut buf = vec![0u8; 2048];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((n, de)) => {
                    if tx.send((buf[..n].to_vec(), de)).is_err() {
                        break;
                    }
                }
                Err(_) => thread::sleep(Duration::from_millis(1)),
            }
        }
    });

    let mut decoder =
        match Decoder::with_api_config(OpenH264API::from_source(), DecoderConfig::default()) {
            Ok(d) => d,
            Err(e) => {
                println!("Falha ao criar o decodificador: {:?}", e);
                return;
            }
        };

    let mut janela: Option<Window> = None;
    let mut dims = (0usize, 0usize);
    let mut pixels: Vec<u32> = Vec::new();
    let mut parcial: Option<Parcial> = None;
    let mut ultimo_id: Option<u32> = None;
    let mut esperando_chave = true;
    let mut remetente: Option<SocketAddr> = par;
    let mut ultimo_pedido: Option<Instant> = None;
    let mut ult_ka = Instant::now();
    let mut ult_seg = Instant::now();
    let mut quadros_vistos = 0u32;
    let mut bytes_rec = 0u64;
    let mut ms_dec = 0.0f64;
    let mut ms_cor = 0.0f64;

    println!("Esperando a transmissao... (ESC na janela para sair)");

    'principal: loop {
        let mut pacotes = Vec::new();
        match rx.recv_timeout(Duration::from_millis(4)) {
            Ok(p) => pacotes.push(p),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
        while let Ok(p) = rx.try_recv() {
            pacotes.push(p);
        }

        for (dados, de) in pacotes {
            // Conectado pela internet: ignora qualquer pacote que nao venha do outro PC.
            if par.is_some() && Some(de) != par {
                continue;
            }
            remetente = Some(de);
            bytes_rec += dados.len() as u64;
            if dados.len() < 9 {
                continue;
            }
            let id = u32::from_be_bytes([dados[0], dados[1], dados[2], dados[3]]);
            let idx = u16::from_be_bytes([dados[4], dados[5]]) as usize;
            let total = u16::from_be_bytes([dados[6], dados[7]]) as usize;
            let chave = dados[8] == 1;
            if total == 0 || idx >= total {
                continue;
            }

            let eh_novo = match ultimo_id {
                None => true,
                Some(u) => {
                    let d = id.wrapping_sub(u);
                    if d == 0 {
                        false
                    } else if d < 0x8000_0000 {
                        if d > 1 {
                            esperando_chave = true; // quadro inteiro perdido
                        }
                        true
                    } else {
                        continue; // pacote velho
                    }
                }
            };
            if eh_novo {
                if parcial.is_some() {
                    esperando_chave = true; // quadro anterior ficou incompleto
                }
                ultimo_id = Some(id);
                parcial = Some(Parcial {
                    total,
                    recebidos: 0,
                    pedacos: vec![None; total],
                    chave,
                });
            }

            let p = match parcial.as_mut() {
                Some(p) => p,
                None => continue,
            };
            if p.total != total {
                continue;
            }
            if p.pedacos[idx].is_none() {
                p.pedacos[idx] = Some(dados[9..].to_vec());
                p.recebidos += 1;
            }
            if p.recebidos < p.total {
                continue;
            }

            let p = parcial.take().unwrap();
            let eh_chave = p.chave;
            let mut h264 = Vec::new();
            for ped in p.pedacos.into_iter().flatten() {
                h264.extend_from_slice(&ped);
            }
            if esperando_chave && !eh_chave {
                continue;
            }

            let t_dec = Instant::now();
            let resultado = decoder.decode(&h264);
            ms_dec += t_dec.elapsed().as_secs_f64() * 1000.0;
            match resultado {
                Ok(Some(yuv)) => {
                    let t_cor = Instant::now();
                    let (w, h) = yuv.dimensions();
                    let strides = yuv.strides();
                    pixels.resize(w * h, 0);
                    yuv_para_pixels(yuv.y(), yuv.u(), yuv.v(), strides, (w, h), &mut pixels);
                    ms_cor += t_cor.elapsed().as_secs_f64() * 1000.0;
                    if eh_chave {
                        esperando_chave = false;
                    }
                    quadros_vistos += 1;

                    if janela.is_none() || dims != (w, h) {
                        janela = match Window::new(
                            "Assistindo - ESC para sair",
                            w,
                            h,
                            WindowOptions {
                                resize: true,
                                scale_mode: ScaleMode::AspectRatioStretch,
                                ..WindowOptions::default()
                            },
                        ) {
                            Ok(mut j) => {
                                j.set_target_fps(60);
                                Some(j)
                            }
                            Err(e) => {
                                println!("Erro ao abrir a janela: {}", e);
                                break 'principal;
                            }
                        };
                    }
                    dims = (w, h);
                }
                Ok(None) => {}
                Err(_) => esperando_chave = true,
            }
        }

        if esperando_chave {
            if let Some(dest) = remetente {
                let pode = match ultimo_pedido {
                    None => true,
                    Some(t) => t.elapsed() >= Duration::from_millis(500),
                };
                if pode {
                    let _ = socket_resp.send_to(b"K", dest);
                    ultimo_pedido = Some(Instant::now());
                }
            }
        }

        // Mantem o caminho aberto no roteador de quem assiste.
        if ult_ka.elapsed() >= Duration::from_secs(5) {
            if let Some(dest) = remetente {
                let _ = socket_resp.send_to(b"P", dest);
            }
            ult_ka = Instant::now();
        }

        if let Some(j) = janela.as_mut() {
            if !j.is_open() || j.is_key_down(Key::Escape) {
                break;
            }
            if j.update_with_buffer(&pixels, dims.0, dims.1).is_err() {
                break;
            }
        }

        if ult_seg.elapsed() >= Duration::from_secs(1) {
            println!(
                "{} quadros/s, {:.1} Mbps recebidos | em 1 s: decodificar {:.0} ms, cor {:.0} ms",
                quadros_vistos,
                (bytes_rec as f64 * 8.0) / 1_000_000.0,
                ms_dec,
                ms_cor
            );
            quadros_vistos = 0;
            bytes_rec = 0;
            ms_dec = 0.0;
            ms_cor = 0.0;
            ult_seg = Instant::now();
        }
    }
}
