# Wikelist Watcher

`wikelo-watcher.exe` é o programa que roda no PC de quem joga Star Citizen e conversa com um servidor [Wikelist](https://wikelist.net) (o site que acompanha os contratos do **Wikelo**). Ele faz duas coisas:

- **Lê o `Game.log`** enquanto você joga e manda para a sua conta os eventos de missão do Wikelo (aceitou, entregou, concluiu) e o fim das outras missões que pagam itens que o site acompanha, dizendo se a missão era sua ou compartilhada por outro jogador (linha `<MissionShared>`). O site ajusta seu inventário e registra as conclusões sozinho.
- **Envia os dados do jogo** quando sai uma versão nova (opcional): extrai os contratos, itens e ícones do `Data.p4k` instalado e sobe um pacote para o servidor, que valida tudo em quarentena antes de usar.

Este repositório é público para que qualquer pessoa possa ler o que o programa faz e conferir que o executável publicado foi compilado a partir deste código.

## Baixar e conferir

Em [Releases](https://github.com/lfccs/wikelist-watcher/releases) está o `wikelo-watcher.exe` e um `SHA256SUMS.txt`, os dois gerados pelo GitHub Actions a partir da tag ([`.github/workflows/release.yml`](.github/workflows/release.yml)). Depois de baixar:

```powershell
Get-FileHash .\wikelo-watcher.exe -Algorithm SHA256   # tem que bater com SHA256SUMS.txt
gh attestation verify .\wikelo-watcher.exe --repo lfccs/wikelist-watcher   # GitHub assina cada build (precisa do GitHub CLI)
.\wikelo-watcher.exe hash                              # mostra o commit embutido no binário
.\wikelo-watcher.exe status                            # mostra a versão e avisa quando há uma mais nova
```

O exe não tem assinatura de código (SmartScreen pode avisar na primeira execução: *Mais informações → Executar assim mesmo*); a atestação acima é a prova de origem. O watcher pergunta ao servidor, uma vez por dia, qual é a release mais recente e avisa na janela e no `status` quando há uma mais nova; o site mostra o mesmo em **Dados → Watcher**.

Dois cliques abrem a **janela de opções**: servidor, usuário e senha da conta do site, **Iniciar/Parar**, o canal seguido (LIVE, PTU…), **Iniciar junto com o Windows** e a opção **Enviar também os dados do jogo quando sair uma versão nova**. Ao entrar, o watcher já começa a rodar em segundo plano; a janela pode ser fechada.

## Comandos

Os mesmos recursos existem para terminal e scripts:

| Comando | O que faz |
|---|---|
| `wikelo-watcher` | Abre a janela de opções. |
| `wikelo-watcher start [--server URL --handle X --password Y --share-data true\|false --quiet]` | Configura se preciso (pergunta o que faltar) e deixa o watcher rodando em segundo plano. |
| `wikelo-watcher stop` | Para o watcher em segundo plano. |
| `wikelo-watcher status` | Diz se está rodando, servidor/conta/canal, contato com o servidor e o resultado da última checagem de versões. |
| `wikelo-watcher autostart on` / `off` | Sobe junto com o Windows (entrada em `HKCU\…\Run`). |
| `wikelo-watcher watch` | Mesma coisa, mas na janela atual (Ctrl+C para sair), útil para ver o que acontece. |
| `wikelo-watcher logout` | Para e esquece a sessão. |
| `wikelo-watcher bundle` | Gera o pacote à mão (`wikelo-LIVE-4.10.2.zip` ao lado do exe) para enviar no site. |
| `wikelo-watcher hash` | Extrai e imprime o hash reproduzível do conteúdo de cada canal, para conferir com o que o servidor mostra. |
| `wikelo-watcher installs` / `extract` / `dump` / `p4k-ls` | Ferramentas de extração para quem quer olhar os dados do jogo. |

O executável é um programa gráfico do Windows; num terminal ele se anexa ao console para imprimir, mas o prompt volta na hora (`wikelo-watcher status | more` espera).

## Como funciona

- Em segundo plano ele roda com prioridade abaixo do normal, lê o `Game.log` a cada 0,75 s (só os bytes novos) e manda os eventos de missão para `POST /api/agent/events` do servidor, com um sinal de vida a cada 20 s. A sessão vem de `POST /api/auth/agent-login` (token de longa duração, guardado só no seu PC).
- Com o **envio dos dados do jogo** ativado, a cada hora (e quando a janela pede com *Verificar versões agora*) ele compara as versões instaladas (LIVE, PTU…) com `GET /api/agent/datasets`; só quando aparece uma versão que o servidor não tem ele extrai e envia o pacote, e nunca enquanto o jogo está aberto.
- A extração é **determinística**: o mesmo `Data.p4k` produz sempre o mesmo `wikelo-data.json`, e o pacote carrega um `contentHash` (`v1:<sha256>` do dataset canônico, sem campos de máquina) que o servidor recalcula e mostra ao lado de cada envio. O pacote **não é assinado**: a confiança vem da validação no servidor (processo isolado, dados re-serializados, imagens recodificadas) e desse hash, que qualquer pessoa com o jogo reproduz com `wikelo-watcher hash`.
- Sessão, estado e registro ficam em `%LOCALAPPDATA%\wikelo-sc\data\` (`watcher.json`, `watcher.status.json`, `watcher.log`). Sai do PC apenas o que está descrito acima: eventos de missão do Wikelo (com o nick do personagem que aparece no `Game.log`) e, se você ativar, os dados extraídos do jogo.

## Compilar você mesmo

```powershell
rustup show                                   # instala a versão fixada em rust-toolchain.toml
cargo build --release --locked -p wikelo-watcher
# target\release\wikelo-watcher.exe
```

Leitura do `Data.p4k`/DataForge via [StarBreaker](https://github.com/diogotr7/StarBreaker) (MIT), fixado em um commit em `Cargo.toml`.

## Estrutura

| Crate | O que faz |
|---|---|
| `crates/wikelo-watcher` | O programa: janela (eframe), daemon, extração (`extract.rs`, `normalize.rs`, `images.rs`), pacote (`bundle.rs`), exportação 3D (`glb.rs`). Também é usada como biblioteca pelo servidor. |
| `crates/wikelo-core` | Tipos do dataset, cálculos (faltantes, "dá pra completar"), hash canônico e invariantes de validação (`validate.rs`), histórico de versões. |
| `crates/wikelo-install` | Encontra as instalações (`LIVE`, `PTU`, …) e lê `build_manifest.id`. |
| `crates/wikelo-log` | Parser do `Game.log`, máquina de estados por missão, importador de `logbackups\`, tailer com detecção de rotação. |

O servidor e a interface web ficam em outro repositório, que inclui este como subárvore (`watcher/`).

## Licença

MIT, veja [LICENSE](LICENSE). Star Citizen® é marca da Cloud Imperium Games; projeto pessoal, sem afiliação.
