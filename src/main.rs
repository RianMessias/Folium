//! Folium - leitor de EPUB em Rust, estilo Kindle.
//! Abre EPUBs, pagina o texto e salva o historico (em qual pagina voce parou).

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CHARS_PER_PAGE: usize = 1800;
const APP_NAME: &str = "Folium";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedBook {
    title: String,
    author: String,
    path: String,
    page: usize,
    total: usize,
    updated: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Theme {
    Claro,
    Sepia,
    Escuro,
}

impl Theme {
    fn label(self) -> &'static str {
        match self {
            Theme::Claro => "Claro",
            Theme::Sepia => "Sepia",
            Theme::Escuro => "Escuro",
        }
    }
}

struct App {
    books: HashMap<String, SavedBook>,
    current_id: Option<String>,
    pages: Vec<Page>,
    page_idx: usize,
    font_size: f32,
    theme: Theme,
    status: String,
    history_path: PathBuf,
}

/// Uma pagina do livro: texto paginado ou imagem (mangas/scans de pagina inteira).
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum Page {
    Text(String),
    Image { bytes: Vec<u8>, w: u32, h: u32 },
}

impl App {
    fn new(_cc: &eframe::CreationContext) -> Self {
        let history_path = history_file();
        let books = load_history(&history_path);
        // reabre o ultimo lido, se ainda existir
        let mut current_id = None;
        let mut pages = Vec::new();
        let mut page_idx = 0;
        if let Some((id, saved)) = books.iter().max_by_key(|(_, b)| b.updated.clone()) {
            let p = PathBuf::from(&saved.path);
            if p.exists() {
                match Self::extract_pages(&p) {
                    Ok(pg) => {
                        page_idx = saved.page.min(pg.len().saturating_sub(1));
                        pages = pg;
                        current_id = Some(id.clone());
                    }
                    Err(e) => eprintln!("falha ao reabrir {id}: {e}"),
                }
            }
        }
        Self {
            books,
            current_id,
            pages,
            page_idx,
            font_size: 17.0,
            theme: Theme::Sepia,
            status: String::new(),
            history_path,
        }
    }

    fn book_id(path: &std::path::Path) -> String {
        let meta = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let mut h = Sha256::new();
        h.update(path.to_string_lossy().as_bytes());
        h.update(meta.to_be_bytes());
        format!("{:x}", h.finalize())[..16].to_string()
    }

    fn extract_pages(path: &std::path::Path) -> AnyhowText<Vec<Page>> {
        let mut doc = epub::doc::EpubDoc::new(path).map_err(|e| e.to_string())?;
        let n = doc.get_num_chapters();
        let mut pages: Vec<Page> = Vec::new();
        let mut full = String::new();
        for i in 0..n {
            doc.set_current_chapter(i);
            let base = doc.get_current_path();
            let Some((content, _mime)) = doc.get_current_str() else {
                continue;
            };
            // Capas/paginas de manga (ex. KCC): o xhtml so tem <img> + "." escondido.
            // Cada imagem vira uma pagina; o texto e ignorado nesses capitulos.
            let mut found_img = false;
            for src in extract_img_srcs(&content) {
                let resolved = resolve_href(base.as_deref(), &src);
                if let Some(bytes) = doc.get_resource_by_path(&resolved) {
                    if bytes.is_empty() {
                        continue;
                    }
                    match imagesize::blob_size(&bytes) {
                        Ok(sz) if sz.width > 0 && sz.height > 0 => {
                            pages.push(Page::Image {
                                bytes,
                                w: sz.width as u32,
                                h: sz.height as u32,
                            });
                            found_img = true;
                        }
                        _ => continue,
                    }
                }
            }
            if found_img {
                continue;
            }
            let text = html2text::from_read(content.as_bytes(), 100);
            let t = text.trim();
            // ignora placeholders de paginas fixas (ex. <div style="display:none;">.</div>)
            if t.is_empty() || t == "." {
                continue;
            }
            full.push_str(t);
            full.push_str("\n\n");
        }
        if !full.trim().is_empty() {
            for t in paginate(&full, CHARS_PER_PAGE) {
                pages.push(Page::Text(t));
            }
        }
        if pages.is_empty() {
            return Err("EPUB sem conteudo extraivel".to_string());
        }
        Ok(pages)
    }

    fn open_file(&mut self, path: PathBuf) {
        let path_str = path.to_string_lossy().to_string();
        match Self::extract_pages(&path) {
            Ok(pages) => {
                // metadados
                let (title, author) = read_metadata(&path);
                let id = Self::book_id(&path);
                let saved_page = self.books.get(&id).map(|b| b.page).unwrap_or(0);
                let total = pages.len();
                let page = saved_page.min(total.saturating_sub(1));
                self.books.insert(
                    id.clone(),
                    SavedBook {
                        title: title.clone(),
                        author: author.clone(),
                        path: path_str,
                        page,
                        total,
                        updated: now_iso(),
                    },
                );
                self.current_id = Some(id);
                self.pages = pages;
                self.page_idx = page;
                self.status = format!("Aberto: {title} ({total} pags.)");
                self.save_history();
            }
            Err(e) => self.status = format!("Erro ao abrir: {e}"),
        }
    }

    fn goto(&mut self, idx: usize) {
        if self.pages.is_empty() {
            return;
        }
        self.page_idx = idx.min(self.pages.len() - 1);
        if let Some(id) = self.current_id.clone() {
            if let Some(b) = self.books.get_mut(&id) {
                b.page = self.page_idx;
                b.total = self.pages.len();
                b.updated = now_iso();
            }
            self.save_history();
        }
    }

    fn save_history(&self) {
        if let Some(dir) = self.history_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.books) {
            let _ = std::fs::write(&self.history_path, json);
        }
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        let mut visuals = match self.theme {
            Theme::Claro => egui::Visuals::light(),
            Theme::Sepia => {
                let mut v = egui::Visuals::light();
                v.panel_fill = egui::Color32::from_rgb(244, 236, 216);
                v.window_fill = egui::Color32::from_rgb(244, 236, 216);
                v.extreme_bg_color = egui::Color32::from_rgb(232, 222, 196);
                v
            }
            Theme::Escuro => egui::Visuals::dark(),
        };
        visuals.widgets.inactive.rounding = 6.0.into();
        ctx.set_visuals(visuals);
    }
}

// tipo erro simples sem nova dependencia
type AnyhowText<T> = Result<T, String>;

fn read_metadata(path: &std::path::Path) -> (String, String) {
    let fallback_title = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Sem titulo".to_string());
    let Ok(doc) = epub::doc::EpubDoc::new(path) else {
        return (fallback_title, "Desconhecido".to_string());
    };
    let title = doc.mdata("title").map(|m| m.value.clone()).unwrap_or(fallback_title);
    let author = doc
        .mdata("creator")
        .map(|m| m.value.clone())
        .unwrap_or_else(|| "Desconhecido".to_string());
    (title, author)
}

fn paginate(text: &str, per: usize) -> Vec<String> {
    let mut pages = Vec::new();
    let mut cur = String::new();
    for para in text.split("\n\n") {
        let p = para.trim();
        if p.is_empty() {
            continue;
        }
        if cur.len() + p.len() + 2 > per && !cur.is_empty() {
            // tenta nao cortar no meio de frase: pagina atual fecha aqui
            pages.push(cur.trim().to_string());
            cur = String::new();
        }
        if p.len() > per {
            // paragrafo gigante: quebra por palavras
            let mut chunk = String::new();
            for w in p.split_whitespace() {
                if chunk.len() + w.len() + 1 > per {
                    pages.push(chunk.trim().to_string());
                    chunk = String::new();
                }
                if !chunk.is_empty() {
                    chunk.push(' ');
                }
                chunk.push_str(w);
            }
            if !chunk.trim().is_empty() {
                cur.push_str(chunk.trim());
                cur.push_str("\n\n");
            }
        } else {
            cur.push_str(p);
            cur.push_str("\n\n");
        }
    }
    if !cur.trim().is_empty() {
        pages.push(cur.trim().to_string());
    }
    if pages.is_empty() {
        pages.push(text.trim().to_string());
    }
    pages
}

/// Extrai valores de src="..." de tags <img> (sem regex, sem nova dependencia).
fn extract_img_srcs(xhtml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = xhtml.as_bytes();
    let mut i = 0;
    while i + 4 < bytes.len() {
        let is_src = bytes[i] == b's' && bytes[i + 1] == b'r' && bytes[i + 2] == b'c' && bytes[i + 3] == b'=';
        if is_src {
            let mut j = i + 4;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                let quote = bytes[j];
                j += 1;
                let start = j;
                while j < bytes.len() && bytes[j] != quote {
                    j += 1;
                }
                let src = &xhtml[start..j];
                let low = src.to_ascii_lowercase();
                if low.ends_with(".jpg") || low.ends_with(".jpeg") || low.ends_with(".png") || low.ends_with(".webp") {
                    out.push(src.to_string());
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Resolve href relativo (ex. "../Images/p01.jpg") contra o path do capitulo
/// (ex. "OEBPS/Text/p01.xhtml"). Retorna String com "/" (formato do ZIP,
/// PathBuf usaria "\" no Windows e o lookup falharia).
fn resolve_href(base: Option<&std::path::Path>, href: &str) -> String {
    use std::path::Component;
    let mut parts: Vec<String> = match base.and_then(|b| b.parent()) {
        Some(d) => d
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect(),
        None => Vec::new(),
    };
    for comp in std::path::Path::new(href).components() {
        match comp {
            Component::ParentDir => {
                parts.pop();
            }
            Component::CurDir => {}
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::RootDir | Component::Prefix(_) => {
                parts = vec![comp.as_os_str().to_string_lossy().into_owned()];
            }
        }
    }
    parts.join("/")
}

fn history_file() -> PathBuf {
    if let Some(proj) = directories::ProjectDirs::from("com", "rianmessias", "folium") {
        return proj.data_dir().join("history.json");
    }
    PathBuf::from("history.json")
}

fn load_history(path: &std::path::Path) -> HashMap<String, SavedBook> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn now_iso() -> String {
    // sem chrono: usa epoch como string ordenavel
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("{}", d.as_secs()),
        Err(_) => "0".to_string(),
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_theme(ctx);

        // atalhos de teclado
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::PageDown)) {
            let n = self.page_idx + 1;
            self.goto(n);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp)) {
            self.goto(self.page_idx.saturating_sub(1));
        }

        egui::SidePanel::left("biblioteca").resizable(true).default_width(250.0).show(ctx, |ui| {
            ui.heading("📚 Biblioteca");
            if ui.button("📂 Abrir EPUB...").clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("EPUB", &["epub"]).pick_file() {
                    self.open_file(path);
                }
            }
            ui.separator();
            let mut order: Vec<(String, SavedBook)> =
                self.books.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            order.sort_by(|a, b| b.1.updated.cmp(&a.1.updated));
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (id, b) in order {
                    let pct = if b.total > 0 { b.page * 100 / b.total } else { 0 };
                    let selected = Some(id.clone()) == self.current_id;
                    ui.group(|ui| {
                        ui.label(egui::RichText::new(&b.title).strong());
                        ui.label(egui::RichText::new(&b.author).small().weak());
                        ui.label(egui::RichText::new(format!("pag. {} de {} ({}%)", b.page + 1, b.total.max(1), pct)).small().weak());
                        ui.horizontal(|ui| {
                            if ui.small_button("▶ Continuar").clicked() {
                                let p = PathBuf::from(&b.path);
                                if p.exists() {
                                    match Self::extract_pages(&p) {
                                        Ok(pg) => {
                                            self.current_id = Some(id.clone());
                                            self.pages = pg;
                                            self.page_idx = b.page.min(self.pages.len().saturating_sub(1));
                                            self.status = format!("Retomado: {} (pag. {})", b.title, self.page_idx + 1);
                                        }
                                        Err(e) => self.status = format!("Erro: {e}"),
                                    }
                                } else {
                                    self.status = format!("Arquivo nao encontrado: {}", b.path);
                                }
                            }
                            if ui.small_button("✖").clicked() {
                                self.books.remove(&id);
                                if self.current_id == Some(id.clone()) {
                                    self.current_id = None;
                                    self.pages.clear();
                                    self.page_idx = 0;
                                }
                                self.save_history();
                            }
                        });
                    });
                    let _ = selected;
                }
            });
        });

        egui::TopBottomPanel::bottom("nav").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("⏮ Anterior").clicked() {
                    self.goto(self.page_idx.saturating_sub(1));
                }
                let total = self.pages.len().max(1);
                let mut p = self.page_idx;
                let resp = ui.add(egui::Slider::new(&mut p, 0..=total - 1).text("pagina").show_value(false));
                if resp.changed() {
                    self.goto(p);
                }
                if ui.button("Proxima ⏭").clicked() {
                    self.goto(self.page_idx + 1);
                }
                let pct = if self.pages.is_empty() { 0 } else { (self.page_idx + 1) * 100 / total };
                ui.label(format!("{} / {} ({}%)", self.page_idx + 1, total, pct));
                ui.separator();
                if ui.small_button("A-").clicked() {
                    self.font_size = (self.font_size - 1.0).max(12.0);
                }
                if ui.small_button("A+").clicked() {
                    self.font_size = (self.font_size + 1.0).min(30.0);
                }
                egui::ComboBox::from_label("Tema").selected_text(self.theme.label()).show_ui(ui, |ui| {
                    for t in [Theme::Claro, Theme::Sepia, Theme::Escuro] {
                        ui.selectable_value(&mut self.theme, t, t.label());
                    }
                });
            });
            if !self.status.is_empty() {
                ui.label(egui::RichText::new(&self.status).small().weak());
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(id) = self.current_id.clone() {
                if let Some(b) = self.books.get(&id) {
                    ui.heading(&b.title);
                    ui.label(egui::RichText::new(&b.author).weak());
                    ui.separator();
                }
                egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                    match self.pages.get(self.page_idx).cloned() {
                        Some(Page::Image { bytes, .. }) => {
                            // pagina de manga: imagem centralizada, ajustada a altura
                            ui.vertical_centered(|ui| {
                                let uri = format!(
                                    "bytes://{}-{}",
                                    self.current_id.clone().unwrap_or_default(),
                                    self.page_idx
                                );
                                ui.add(
                                    egui::Image::from_bytes(uri, bytes)
                                        .shrink_to_fit()
                                        .max_height(ui.available_height()),
                                );
                            });
                        }
                        Some(Page::Text(texto)) => {
                            // coluna central estilo Kindle
                            ui.horizontal(|ui| {
                                let w = (ui.available_width() - 560.0).max(0.0) / 2.0;
                                ui.add_space(w);
                                ui.vertical(|ui| {
                                    ui.set_width(560.0f32.min(ui.available_width()));
                                    ui.label(egui::RichText::new(texto).size(self.font_size).line_height(Some(self.font_size * 1.6)));
                                });
                            });
                        }
                        None => {
                            ui.label("Nenhuma pagina.");
                        }
                    }
                });
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.heading("Folium 📖");
                    ui.label("Abra um EPUB na barra lateral para comecar.");
                    ui.label("Seu progresso (pagina atual) e salvo automaticamente.");
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn img_src_e_resolve() {
        let x = r#"<html><body><img width="1" src="../Images/kcc-0001-kcc-x.jpg"/><img src='a.png'/></body></html>"#;
        let srcs = extract_img_srcs(x);
        assert_eq!(srcs, vec!["../Images/kcc-0001-kcc-x.jpg".to_string(), "a.png".to_string()]);
        let base = Some(std::path::Path::new("OEBPS/Text/kcc-0001.xhtml"));
        assert_eq!(
            resolve_href(base, "../Images/kcc-0001-kcc-x.jpg"),
            "OEBPS/Images/kcc-0001-kcc-x.jpg".to_string()
        );
    }

    /// Teste de integracao local (pula se o arquivo nao existir): manga KCC deve
    /// gerar 1 pagina de imagem por figura, nao 1 pagina de ".".
    #[test]
    fn manga_gera_paginas_de_imagem() {
        let p = std::path::Path::new(r"E:\Mangas\Dororo\Kindle\Dororo Cap 01 - Osamu Tezuka.epub");
        if !p.exists() {
            eprintln!("pulando: epub de teste ausente");
            return;
        }
        let pages = App::extract_pages(p).expect("extracao falhou");
        let imgs = pages.iter().filter(|p| matches!(p, Page::Image { .. })).count();
        assert!(imgs >= 150, "esperava 150+ paginas de imagem, achou {imgs} em {} pags", pages.len());
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 750.0]),
        ..Default::default()
    };
    eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
