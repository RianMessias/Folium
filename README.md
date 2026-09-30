# Folium

Leitor de EPUB estilo Kindle para PC, em Rust (rapido e liso).

- Abre `.epub` com dialogo nativo (texto e **mangas de pagina inteira**)
- Importa pasta completa com EPUBs (recursivo, pula duplicados)
- Pagina o texto (coluna central, temas Claro/Sepia/Escuro, fonte A-/A+)
- Mangas KCC/fixed-layout: cada imagem vira uma pagina, ajustada a tela
- Biblioteca: capas (com fallback p/ 1a imagem), pastas/colecoes (arraste um livro sobre outro para criar,
  nome obrigatorio), progresso por livro
- Importar pasta: recursivo, ignora duplicados (mesmo titulo/autor/paginas), botao "Limpar duplicados"
- **Historico**: salva automaticamente em qual pagina voce parou por livro e retoma ao abrir

## Rodar

```bash
cargo run --release
```

## Atalhos

- `→` / `PageDown` / botão Próxima: avança
- `←` / `PageUp` / botão Anterior: volta
- Slider inferior: pula de pagina

## Historico

Fica em `%APPDATA%/folium/history.json` (via `directories`), um registro por livro com pagina atual, total e data.
