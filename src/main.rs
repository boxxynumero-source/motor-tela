mod assistir;
mod compartilhar;

use std::io::{self, Write};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Servidores STUN gratuitos (so dizem qual e o seu endereco na internet).
const SERVIDORES_STUN: [&str; 3] = [
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

const COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];

fn perguntar(texto: &str, padrao: &str) -> String {
    if padrao.is_empty() {
        print!("{}: ", texto);
    } else {
        print!("{} [{}]: ", texto, padrao);
    }
    let _ = io::stdout().flush();
    let mut linha = String::new();
    let _ = io::stdin().read_line(&mut linha);
    let t = linha.trim();
    if t.is_empty() {
        padrao.to_string()
    } else {
        t.to_string()
    }
}

fn pausar() {
    print!("Aperte Enter para fechar.");
    let _ = io::stdout().flush();
    let mut linha = String::new();
    let _ = io::stdin().read_line(&mut linha);
}

fn novo_id() -> [u8; 12] {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(1);
    let mut id = [0u8; 12];
    id.copy_from_slice(&n.to_be_bytes()[4..16]);
    id
}

fn pedido_stun(id: &[u8; 12]) -> [u8; 20] {
    let mut p = [0u8; 20];
    p[0] = 0x00;
    p[1] = 0x01; // Binding Request
    p[4..8].copy_from_slice(&COOKIE);
    p[8..20].copy_from_slice(id);
    p
}

fn ler_resposta(dados: &[u8], id: &[u8; 12]) -> Option<SocketAddr> {
    if dados.len() < 20 {
        return None;
    }
    if dados[0] != 0x01 || dados[1] != 0x01 {
        return None;
    }
    if dados[4..8] != COOKIE[..] {
        return None;
    }
    if dados[8..20] != id[..] {
        return None;
    }
    let mut pos = 20;
    let mut achado = None;
    while pos + 4 <= dados.len() {
        let tipo = u16::from_be_bytes([dados[pos], dados[pos + 1]]);
        let tam = u16::from_be_bytes([dados[pos + 2], dados[pos + 3]]) as usize;
        let ini = pos + 4;
        if ini + tam > dados.len() {
            break;
        }
        let v = &dados[ini..ini + tam];
        if (tipo == 0x0020 || tipo == 0x0001) && tam >= 8 && v[1] == 0x01 {
            let (porta, ip) = if tipo == 0x0020 {
                (
                    u16::from_be_bytes([v[2], v[3]]) ^ 0x2112,
                    [v[4] ^ COOKIE[0], v[5] ^ COOKIE[1], v[6] ^ COOKIE[2], v[7] ^ COOKIE[3]],
                )
            } else {
                (u16::from_be_bytes([v[2], v[3]]), [v[4], v[5], v[6], v[7]])
            };
            let a = SocketAddr::from((ip, porta));
            if tipo == 0x0020 {
                return Some(a);
            }
            achado = Some(a);
        }
        pos = ini + ((tam + 3) & !3);
    }
    achado
}

// Devolve (meu endereco na internet, servidor STUN que respondeu).
fn descobrir_endereco(socket: &UdpSocket) -> Option<(SocketAddr, SocketAddr)> {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(1500)));
    let mut achou = None;
    'servidores: for nome in SERVIDORES_STUN.iter() {
        let mut enderecos = match nome.to_socket_addrs() {
            Ok(e) => e,
            Err(_) => continue,
        };
        let servidor = match enderecos.find(|a| a.is_ipv4()) {
            Some(s) => s,
            None => continue,
        };
        for _tentativa in 0..2 {
            let id = novo_id();
            if socket.send_to(&pedido_stun(&id), servidor).is_err() {
                continue;
            }
            let mut buf = [0u8; 512];
            if let Ok((n, de)) = socket.recv_from(&mut buf) {
                if de == servidor {
                    if let Some(a) = ler_resposta(&buf[..n], &id) {
                        achou = Some((a, servidor));
                        break 'servidores;
                    }
                }
            }
        }
    }
    let _ = socket.set_read_timeout(None);
    achou
}

// Os dois PCs mandam pacotes pequenos um para o outro ao mesmo tempo.
// Isso faz o roteador de cada um aceitar a resposta do outro.
fn furar(socket: &UdpSocket, amigo: SocketAddr) -> Option<SocketAddr> {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(300)));
    let inicio = Instant::now();
    let mut ultimo_aviso = 0u64;
    let mut buf = [0u8; 2048];
    let mut par = None;
    while inicio.elapsed() < Duration::from_secs(60) {
        let _ = socket.send_to(b"P", amigo);
        if let Ok((_n, de)) = socket.recv_from(&mut buf) {
            if de.ip() == amigo.ip() {
                par = Some(de);
                break;
            }
        }
        let s = inicio.elapsed().as_secs();
        if s >= ultimo_aviso + 5 {
            println!("Tentando conectar... {} s", s);
            ultimo_aviso = s;
        }
    }
    if let Some(p) = par {
        for _ in 0..3 {
            let _ = socket.send_to(b"P", p);
            thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = socket.set_read_timeout(None);
    par
}

fn conectar_internet() -> Option<(UdpSocket, SocketAddr)> {
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            println!("Nao consegui abrir a rede: {}", e);
            return None;
        }
    };
    println!("Descobrindo o seu endereco na internet...");
    let (meu, servidor) = match descobrir_endereco(&socket) {
        Some(x) => x,
        None => {
            println!("Nao consegui falar com o servidor STUN.");
            println!("Sem internet, ou a rede bloqueia. Tente a opcao 2 (IP direto).");
            return None;
        }
    };

    // Enquanto voce troca os enderecos, mantem o caminho aberto no roteador.
    let parar = Arc::new(AtomicBool::new(false));
    if let Ok(s2) = socket.try_clone() {
        let parar2 = parar.clone();
        thread::spawn(move || {
            while !parar2.load(Ordering::Relaxed) {
                let _ = s2.send_to(&pedido_stun(&novo_id()), servidor);
                for _ in 0..50 {
                    if parar2.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
    }

    println!();
    println!("SEU ENDERECO: {}", meu);
    println!("Mande esse endereco para o seu amigo e peca o dele.");
    println!("Nao feche esta janela enquanto isso.");
    let texto = perguntar("Endereco do seu amigo (exemplo 200.10.20.30:54321)", "");
    parar.store(true, Ordering::Relaxed);
    let amigo: SocketAddr = match texto.parse() {
        Ok(a) => a,
        Err(_) => {
            println!("Endereco invalido. Precisa ser assim: 200.10.20.30:54321");
            return None;
        }
    };

    println!("Conectando por ate 60 segundos. Seu amigo precisa digitar o seu endereco agora.");
    match furar(&socket, amigo) {
        Some(par) => {
            println!("Conectado com {}!", par);
            Some((socket, par))
        }
        None => {
            println!("Nao conectou em 60 segundos.");
            println!("Confira se os dois enderecos estao certos e se os dois estao tentando ao mesmo tempo.");
            println!("Se estiver tudo certo, um dos roteadores pode nao permitir (CGNAT ou NAT restrito).");
            None
        }
    }
}

fn modo_compartilhar(via: &str) {
    let mbps = perguntar("Bitrate em Mbps", "8");
    if via == "2" {
        let ip = perguntar("IP do PC que vai assistir", "127.0.0.1");
        let porta = perguntar("Porta", "5000");
        let destino: SocketAddr = match format!("{}:{}", ip, porta).parse() {
            Ok(d) => d,
            Err(_) => {
                println!("IP ou porta invalidos. Exemplo de IP: 192.168.0.10");
                return;
            }
        };
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(e) => {
                println!("Nao consegui abrir a rede: {}", e);
                return;
            }
        };
        compartilhar::executar(socket, destino, &mbps);
    } else if let Some((socket, par)) = conectar_internet() {
        compartilhar::executar(socket, par, &mbps);
    }
}

fn modo_assistir(via: &str) {
    if via == "2" {
        let porta = perguntar("Porta para receber", "5000");
        let socket = match UdpSocket::bind(format!("0.0.0.0:{}", porta)) {
            Ok(s) => s,
            Err(e) => {
                println!("Nao consegui abrir a porta {}: {}", porta, e);
                return;
            }
        };
        assistir::executar(socket, None);
    } else if let Some((socket, par)) = conectar_internet() {
        assistir::executar(socket, Some(par));
    }
}

fn main() {
    println!("=== Motor de tela (teste v0.3) ===");
    println!("1 = Compartilhar minha tela");
    println!("2 = Assistir");
    let modo = perguntar("Escolha", "1");
    println!();
    println!("Como conectar?");
    println!("1 = Pela internet (cada um digita o endereco do outro)");
    println!("2 = IP direto (mesma rede ou teste neste PC)");
    let via = perguntar("Escolha", "1");
    println!();
    if modo == "2" {
        modo_assistir(&via);
    } else {
        modo_compartilhar(&via);
    }
    pausar();
}
