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
    pages: Vec<String>,
    page_idx: usize,
    font_size: f32,
    theme: Theme,
    status: String,
    history_path: PathBuf,
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

    fn extract_pages(path: &std::path::Path) -> AnyhowText<Vec<String>> {
        let mut doc = epub::doc::EpubDoc::new(path).map_err(|e| e.to_string())?;
        let n = doc.get_num_chapters();
        let mut full = String::new();
        for i in 0..n {
            doc.set_current_chapter(i);
            if let Some((content, _mime)) = doc.get_current_str() {
                let text = html2text::from_read(content.as_bytes(), 100);
                let t = text.trim();
                if !t.is_empty() {
                    full.push_str(t);
                    full.push_str("\n\n");
                }
            }
        }
        if full.trim().is_empty() {
            return Err("EPUB sem texto extraivel (pode ser so imagens/scans)".to_string());
        }
        Ok(paginate(&full, CHARS_PER_PAGE))
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
                    // coluna central estilo Kindle
                    ui.horizontal(|ui| {
                        let w = (ui.available_width() - 560.0).max(0.0) / 2.0;
                        ui.add_space(w);
                        ui.vertical(|ui| {
                            ui.set_width(560.0f32.min(ui.available_width()));
                            let texto = self.pages.get(self.page_idx).cloned().unwrap_or_default();
                            ui.label(egui::RichText::new(texto).size(self.font_size).line_height(Some(self.font_size * 1.6)));
                        });
                    });
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

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 750.0]),
        ..Default::default()
    };
    eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
