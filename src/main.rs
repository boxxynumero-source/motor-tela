mod assistir;
mod compartilhar;

use std::io::{self, Write};

fn perguntar(texto: &str, padrao: &str) -> String {
    print!("{} [{}]: ", texto, padrao);
    io::stdout().flush().ok();
    let mut linha = String::new();
    io::stdin().read_line(&mut linha).ok();
    let linha = linha.trim().to_string();
    if linha.is_empty() {
        padrao.to_string()
    } else {
        linha
    }
}

fn main() {
    println!("=== Motor de tela (teste v0.2) ===");
    println!("1 = Compartilhar minha tela");
    println!("2 = Assistir");
    let modo = perguntar("Escolha", "1");

    if modo == "2" {
        let porta = perguntar("Porta para receber", "5000");
        assistir::executar(&porta);
    } else {
        let ip = perguntar("IP do PC que vai assistir", "127.0.0.1");
        let porta = perguntar("Porta", "5000");
        let mbps = perguntar("Bitrate em Mbps", "8");
        compartilhar::executar(&ip, &porta, &mbps);
    }

    println!("Encerrado. Aperte Enter para fechar.");
    let mut x = String::new();
    io::stdin().read_line(&mut x).ok();
}
