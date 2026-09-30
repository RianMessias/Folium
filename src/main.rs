#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
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

/// Pasta/colecao da biblioteca (estilo Kindle).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct Collection {
    name: String,
    books: Vec<String>, // ids
    #[serde(default)]
    collapsed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[allow(dead_code)]
struct LibraryFile {
    #[serde(default)]
    books: HashMap<String, SavedBook>,
    #[serde(default)]
    collections: Vec<Collection>,
}

/// Alvo de um drop na biblioteca.
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum DropTarget {
    Book(String),
    Collection(usize),
}

/// Estado do modal de nome obrigatorio da nova pasta.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
struct NamingState {
    target: String,  // livro que recebeu o drop
    dragged: String, // livro arrastado
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Theme {
    Claro,
    Sepia,
    Escuro,
    Github,
    Monokai,
    TokyoNight,
}

impl Theme {
    fn label(self) -> &'static str {
        match self {
            Theme::Claro => "Claro",
            Theme::Sepia => "Sepia",
            Theme::Escuro => "Escuro",
            Theme::Github => "GitHub",
            Theme::Monokai => "Monokai",
            Theme::TokyoNight => "Tokyo Night",
        }
    }

    const ALL: [Theme; 6] = [
        Theme::Claro,
        Theme::Sepia,
        Theme::Escuro,
        Theme::Github,
        Theme::Monokai,
        Theme::TokyoNight,
    ];
}

/// Config persistida (preset de cor etc.), em config.json ao lado do history.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    theme: Theme,
}

impl Default for Config {
    fn default() -> Self {
        Self { theme: Theme::Sepia }
    }
}

fn config_path() -> PathBuf {
    if let Some(proj) = directories::ProjectDirs::from("com", "rianmessias", "folium") {
        return proj.data_dir().join("config.json");
    }
    PathBuf::from("config.json")
}

fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &Config) {
    let p = config_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&p, json);
    }
}

struct App {
    books: HashMap<String, SavedBook>,
    collections: Vec<Collection>,
    current_id: Option<String>,
    pages: Vec<Page>,
    page_idx: usize,
    font_size: f32,
    theme: Theme,
    status: String,
    history_path: PathBuf,
    /// Texturas das paginas de imagem ja decodificadas (cache curto p/ nao estourar RAM).
    tex_cache: HashMap<usize, egui::TextureHandle>,
    tex_book: Option<String>,
    /// Miniaturas das capas na biblioteca.
    covers: HashMap<String, egui::TextureHandle>,
    covers_missing: std::collections::HashSet<String>,
    /// Livro sendo arrastado (id) + retangulos de drop registrados neste frame.
    dragging: Option<String>,
    drop_rects: HashMap<String, egui::Rect>,
    /// Modal de nome obrigatorio ao criar pasta.
    naming: Option<NamingState>,
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
        let lib = load_library(&history_path);
        let books = lib.books;
        let mut collections = lib.collections;
        // limpa referencias a livros que nao existem mais
        for c in &mut collections {
            c.books.retain(|id| books.contains_key(id));
        }
        collections.retain(|c| c.books.len() >= 2);
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
            collections,
            current_id,
            pages,
            page_idx,
            font_size: 17.0,
            theme: load_config().theme,
            status: String::new(),
            history_path,
            tex_cache: HashMap::new(),
            tex_book: None,
            covers: HashMap::new(),
            covers_missing: std::collections::HashSet::new(),
            dragging: None,
            drop_rects: HashMap::new(),
            naming: None,
        }
    }

    /// Retorna a textura da pagina de imagem atual, decodificando sob demanda.
    /// Decodifica com a crate `image` (nao depende do loader interno do egui).
    fn page_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let id = self.current_id.clone()?;
        if self.tex_book.as_ref() != Some(&id) {
            self.tex_cache.clear();
            self.tex_book = Some(id.clone());
        }
        if let Some(tex) = self.tex_cache.get(&self.page_idx) {
            return Some(tex.clone());
        }
        let (bytes, w, h) = match self.pages.get(self.page_idx) {
            Some(Page::Image { bytes, w, h }) => (bytes.clone(), *w, *h),
            _ => return None,
        };
        let img = image::load_from_memory(&bytes).ok()?;
        let rgba = img.to_rgba8();
        let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_flat_samples().as_slice());
        // cache curto: so as ultimas paginas visitadas
        if self.tex_cache.len() >= 6 {
            self.tex_cache.clear();
        }
        let tex = ctx.load_texture(format!("folium-{id}-{}", self.page_idx), color, egui::TextureOptions::LINEAR);
        self.tex_cache.insert(self.page_idx, tex.clone());
        Some(tex)
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
        let lib = LibraryFile {
            books: self.books.clone(),
            collections: self.collections.clone(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&lib) {
            let _ = std::fs::write(&self.history_path, json);
        }
    }

    /// Miniatura da capa p/ a biblioteca (cache; None = sem capa ou falhou).
    fn cover_texture(&mut self, ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.covers.get(id) {
            return Some(t.clone());
        }
        if self.covers_missing.contains(id) {
            return None;
        }
        let tex = (|| {
            let path = self.books.get(id).map(|b| b.path.clone())?;
            let bytes = cover_bytes(std::path::Path::new(&path))?;
            let img = image::load_from_memory(&bytes).ok()?;
            let thumb = img.thumbnail(96, 140);
            let rgba = thumb.to_rgba8();
            let (w, h) = (rgba.width() as usize, rgba.height() as usize);
            let color = egui::ColorImage::from_rgba_unmultiplied([w, h], rgba.as_flat_samples().as_slice());
            Some(ctx.load_texture(format!("folium-cover-{id}"), color, egui::TextureOptions::LINEAR))
        })();
        match tex {
            Some(t) => {
                self.covers.insert(id.to_string(), t.clone());
                Some(t)
            }
            None => {
                self.covers_missing.insert(id.to_string());
                None
            }
        }
    }

    /// Em qual pasta (indice) o livro esta, se estiver.
    fn collection_of(&self, book: &str) -> Option<usize> {
        self.collections.iter().position(|c| c.books.iter().any(|b| b == book))
    }

    /// Aplica o drop de um livro sobre um alvo.
    /// Retorna true se abriu o modal de nome (criacao de pasta pendente).
    fn apply_drop(&mut self, dragged: &str, target: &DropTarget) -> bool {
        match target {
            DropTarget::Collection(idx) => {
                if self.collection_of(dragged) == Some(*idx) {
                    return false;
                }
                remove_from_all(&mut self.collections, dragged);
                if let Some(c) = self.collections.get_mut(*idx) {
                    c.books.push(dragged.to_string());
                }
                self.save_history();
                false
            }
            DropTarget::Book(other) => {
                if dragged == other {
                    return false;
                }
                if let Some(idx) = self.collection_of(other) {
                    if self.collection_of(dragged) == Some(idx) {
                        return false;
                    }
                    remove_from_all(&mut self.collections, dragged);
                    if let Some(c) = self.collections.get_mut(idx) {
                        c.books.push(dragged.to_string());
                    }
                    self.save_history();
                    false
                } else {
                    // nova pasta com os dois: abre modal de nome obrigatorio
                    self.naming = Some(NamingState {
                        target: other.clone(),
                        dragged: dragged.to_string(),
                        name: String::new(),
                    });
                    true
                }
            }
        }
    }

    /// Abre o livro na pagina salva e mostra no leitor.
    fn resume_book(&mut self, id: &str) {
        let b = match self.books.get(id) {
            Some(b) => b.clone(),
            None => return,
        };
        let p = PathBuf::from(&b.path);
        if !p.exists() {
            self.status = format!("Arquivo nao encontrado: {}", b.path);
            return;
        }
        match Self::extract_pages(&p) {
            Ok(pg) => {
                self.current_id = Some(id.to_string());
                self.pages = pg;
                self.page_idx = b.page.min(self.pages.len().saturating_sub(1));
                self.status = format!("Retomado: {} (pag. {})", b.title, self.page_idx + 1);
            }
            Err(e) => self.status = format!("Erro: {e}"),
        }
    }

    /// Remove o livro da biblioteca (e das pastas).
    fn delete_book(&mut self, id: &str) {
        self.books.remove(id);
        self.covers.remove(id);
        self.covers_missing.remove(id);
        remove_from_all(&mut self.collections, id);
        if self.current_id.as_deref() == Some(id) {
            self.current_id = None;
            self.pages.clear();
            self.page_idx = 0;
        }
        self.save_history();
    }

    /// Acha alvo de drop na posicao (exclui o proprio livro arrastado).
    fn drop_target_at(&self, pos: egui::Pos2, dragged: &str) -> Option<DropTarget> {
        let self_key = format!("book:{dragged}");
        for (key, rect) in &self.drop_rects {
            if key == &self_key || !rect.contains(pos) {
                continue;
            }
            if let Some(id) = key.strip_prefix("book:") {
                return Some(DropTarget::Book(id.to_string()));
            }
            if let Some(idx) = key.strip_prefix("col:").and_then(|s| s.parse::<usize>().ok()) {
                return Some(DropTarget::Collection(idx));
            }
        }
        None
    }

    /// Linha de um livro na biblioteca: capa + infos + botoes. Arrasta pela capa/titulo.
    fn book_row(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, id: &str, b: &SavedBook, in_collection: bool) {
        let pct = if b.total > 0 { b.page * 100 / b.total } else { 0 };
        let cover_h = 110.0;
        let group = ui.group(|ui| {
            ui.horizontal(|ui| {
                // capa (arrastavel)
                let cover_resp = if let Some(tex) = self.cover_texture(ctx, id) {
                    ui.add(egui::Image::new(&tex).max_height(cover_h).sense(egui::Sense::drag()))
                } else {
                    ui.add(egui::Label::new("📕").sense(egui::Sense::drag()))
                };
                self.handle_drag(ctx, &cover_resp, id);
                ui.vertical(|ui| {
                    ui.set_min_size(egui::vec2(120.0, cover_h));
                    let title_resp = ui.add(
                        egui::Label::new(egui::RichText::new(&b.title).strong()).sense(egui::Sense::drag()),
                    );
                    self.handle_drag(ctx, &title_resp, id);
                    ui.label(egui::RichText::new(&b.author).small().weak());
                    ui.label(
                        egui::RichText::new(format!("pag. {} de {} ({}%)", b.page + 1, b.total.max(1), pct))
                            .small()
                            .weak(),
                    );
                    ui.horizontal(|ui| {
                        if ui.small_button("▶ Continuar").clicked() {
                            self.resume_book(id);
                        }
                        if in_collection && ui.small_button("⏏").on_hover_text("Tirar da pasta").clicked() {
                            remove_from_all(&mut self.collections, id);
                            self.save_history();
                            self.status = "Livro fora da pasta.".to_string();
                        }
                        if ui.small_button("✖").on_hover_text("Excluir da biblioteca").clicked() {
                            self.delete_book(id);
                        }
                    });
                });
            });
        });
        // registra area de drop
        self.drop_rects.insert(format!("book:{id}"), group.response.rect);
    }

    fn handle_drag(&mut self, ctx: &egui::Context, resp: &egui::Response, id: &str) {
        if resp.drag_started() {
            self.dragging = Some(id.to_string());
        }
        if resp.drag_stopped() {
            self.dragging = None;
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if let Some(target) = self.drop_target_at(pos, id) {
                    let dragged = id.to_string();
                    self.apply_drop(&dragged, &target);
                }
            }
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
            Theme::Github => {
                // GitHub Primer light
                let mut v = egui::Visuals::light();
                v.panel_fill = egui::Color32::from_rgb(246, 248, 250);
                v.window_fill = egui::Color32::WHITE;
                v.extreme_bg_color = egui::Color32::from_rgb(234, 238, 242);
                v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(31, 35, 40));
                v.selection.bg_fill = egui::Color32::from_rgb(9, 105, 218);
                v.hyperlink_color = egui::Color32::from_rgb(9, 105, 218);
                v
            }
            Theme::Monokai => {
                // Monokai Pro-ish
                let mut v = egui::Visuals::dark();
                v.panel_fill = egui::Color32::from_rgb(39, 40, 34);
                v.window_fill = egui::Color32::from_rgb(39, 40, 34);
                v.extreme_bg_color = egui::Color32::from_rgb(30, 31, 28);
                v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(248, 248, 242));
                v.selection.bg_fill = egui::Color32::from_rgb(249, 38, 114);
                v.hyperlink_color = egui::Color32::from_rgb(102, 217, 239);
                v.warn_fg_color = egui::Color32::from_rgb(253, 151, 31);
                v
            }
            Theme::TokyoNight => {
                // Tokyo Night
                let mut v = egui::Visuals::dark();
                v.panel_fill = egui::Color32::from_rgb(26, 27, 38);
                v.window_fill = egui::Color32::from_rgb(26, 27, 38);
                v.extreme_bg_color = egui::Color32::from_rgb(22, 22, 30);
                v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(192, 202, 245));
                v.selection.bg_fill = egui::Color32::from_rgb(122, 162, 247);
                v.hyperlink_color = egui::Color32::from_rgb(125, 207, 255);
                v.warn_fg_color = egui::Color32::from_rgb(224, 175, 104);
                v
            }
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

/// Seta do dropdown desenhada à mão (os glifos ▸/▾ não existem na fonte padrão e viram quadrado).
fn dropdown_button(ui: &mut egui::Ui, collapsed: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let c = rect.center();
        let s = 5.0;
        let pts = if collapsed {
            // aponta p/ direita
            vec![
                c + egui::vec2(-s * 0.6, -s),
                c + egui::vec2(-s * 0.6, s),
                c + egui::vec2(s * 0.8, 0.0),
            ]
        } else {
            // aponta p/ baixo
            vec![
                c + egui::vec2(-s, -s * 0.6),
                c + egui::vec2(s, -s * 0.6),
                c + egui::vec2(0.0, s * 0.8),
            ]
        };
        let color = if resp.hovered() {
            ui.visuals().widgets.hovered.fg_stroke.color
        } else {
            ui.visuals().text_color()
        };
        ui.painter().add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
    }
    resp.on_hover_text("Recolher/expandir")
}

fn history_file() -> PathBuf {
    if let Some(proj) = directories::ProjectDirs::from("com", "rianmessias", "folium") {
        return proj.data_dir().join("history.json");
    }
    PathBuf::from("history.json")
}

fn load_library(path: &std::path::Path) -> LibraryFile {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    if raw.trim().is_empty() {
        return LibraryFile::default();
    }
    // formato novo
    if let Ok(lib) = serde_json::from_str::<LibraryFile>(&raw) {
        return lib;
    }
    // migracao do formato antigo (so livros)
    if let Ok(books) = serde_json::from_str::<HashMap<String, SavedBook>>(&raw) {
        return LibraryFile { books, collections: Vec::new() };
    }
    LibraryFile::default()
}

/// Capa do EPUB (bytes da imagem), se houver.
fn cover_bytes(path: &std::path::Path) -> Option<Vec<u8>> {
    let mut doc = epub::doc::EpubDoc::new(path).ok()?;
    doc.get_cover().map(|(bytes, _mime)| bytes)
}

/// Tira o livro de todas as pastas. Dissolve pastas que ficarem com < 2 livros.
fn remove_from_all(collections: &mut Vec<Collection>, book: &str) {
    for c in collections.iter_mut() {
        c.books.retain(|b| b != book);
    }
    collections.retain(|c| c.books.len() >= 2);
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

        egui::SidePanel::left("biblioteca").resizable(true).default_width(260.0).show(ctx, |ui| {
            ui.heading("📚 Biblioteca");
            if ui.button("📂 Abrir EPUB...").clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("EPUB", &["epub"]).pick_file() {
                    self.open_file(path);
                }
            }
            ui.small("Arraste um livro pela capa/titulo sobre outro para criar pasta.");
            ui.separator();
            self.drop_rects.clear();
            // pastas
            let mut desfazer: Option<usize> = None;
            let cols = self.collections.clone();
            for (idx, col) in cols.iter().enumerate() {
                let header = ui.horizontal(|ui| {
                    if dropdown_button(ui, col.collapsed).clicked() {
                        if let Some(c) = self.collections.get_mut(idx) {
                            c.collapsed = !c.collapsed;
                            self.save_history();
                        }
                    }
                    let lbl = ui.add(
                        egui::Label::new(egui::RichText::new(format!("📁 {} ({})", col.name, col.books.len())).strong())
                            .sense(egui::Sense::click()),
                    );
                    if lbl.clicked() {
                        if let Some(c) = self.collections.get_mut(idx) {
                            c.collapsed = !c.collapsed;
                            self.save_history();
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Desfazer").on_hover_text("Desfazer pasta (mantem os livros)").clicked() {
                            desfazer = Some(idx);
                        }
                    });
                });
                self.drop_rects.insert(format!("col:{idx}"), header.response.rect);
                if !col.collapsed {
                    for bid in col.books.clone() {
                        if let Some(b) = self.books.get(&bid).cloned() {
                            self.book_row(ctx, ui, &bid, &b, true);
                        }
                    }
                }
                ui.separator();
            }
            if let Some(idx) = desfazer {
                if idx < self.collections.len() {
                    let name = self.collections[idx].name.clone();
                    self.collections.remove(idx);
                    self.save_history();
                    self.status = format!("Pasta \"{name}\" desfeita.");
                }
            }
            // avulsos (fora de pastas), mais recentes primeiro
            let in_any: std::collections::HashSet<&String> =
                self.collections.iter().flat_map(|c| &c.books).collect();
            let mut loose: Vec<(String, SavedBook)> = self
                .books
                .iter()
                .filter(|(k, _)| !in_any.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            loose.sort_by(|a, b| b.1.updated.cmp(&a.1.updated));
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (id, b) in loose {
                    self.book_row(ctx, ui, &id, &b, false);
                }
            });
        });
        // preview flutuante durante o arrasto
        if let Some(drag_id) = self.dragging.clone() {
            if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                let title = self.books.get(&drag_id).map(|b| b.title.clone()).unwrap_or_default();
                egui::Area::new(egui::Id::new("dndpreview"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(pos + egui::vec2(14.0, 14.0))
                    .show(ctx, |ui| {
                        ui.label(egui::RichText::new(format!("📕 {title}")).strong());
                    });
            }
        }
        // modal de nome obrigatorio da nova pasta
        if self.naming.is_some() {
            let mut close = false;
            let mut create = false;
            egui::Window::new("📁 Nova pasta")
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Nome da pasta (obrigatorio):");
                    let resp = ui.text_edit_singleline(&mut self.naming.as_mut().unwrap().name);
                    resp.request_focus();
                    let ok = !self.naming.as_ref().unwrap().name.trim().is_empty();
                    ui.horizontal(|ui| {
                        if ui.add_enabled(ok, egui::Button::new("Criar")).clicked() {
                            create = true;
                        }
                        if ui.button("Cancelar").clicked() {
                            close = true;
                        }
                    });
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) && ok {
                        create = true;
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                });
            if create {
                if let Some(n) = self.naming.take() {
                    let name = n.name.trim().to_string();
                    remove_from_all(&mut self.collections, &n.target);
                    remove_from_all(&mut self.collections, &n.dragged);
                    self.collections.push(Collection {
                        name: name.clone(),
                        books: vec![n.target, n.dragged],
                        collapsed: false,
                    });
                    self.save_history();
                    self.status = format!("Pasta \"{name}\" criada.");
                }
            } else if close {
                self.naming = None;
            }
        }

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
                let theme_before = self.theme;
                egui::ComboBox::from_label("Tema").selected_text(self.theme.label()).show_ui(ui, |ui| {
                    for t in Theme::ALL {
                        ui.selectable_value(&mut self.theme, t, t.label());
                    }
                });
                if self.theme != theme_before {
                    save_config(&Config { theme: self.theme });
                }
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
                    let is_image = matches!(self.pages.get(self.page_idx), Some(Page::Image { .. }));
                    if is_image {
                        // pagina de manga: imagem centralizada, ajustada a altura
                        let tex = self.page_texture(ctx);
                        ui.vertical_centered(|ui| {
                            match tex {
                                Some(t) => {
                                    ui.add(egui::Image::new(&t).shrink_to_fit().max_height(ui.available_height()));
                                }
                                None => {
                                    ui.label("⚠ nao foi possivel decodificar esta imagem.");
                                }
                            }
                        });
                    } else if let Some(Page::Text(texto)) = self.pages.get(self.page_idx).cloned() {
                            // coluna central estilo Kindle
                            ui.horizontal(|ui| {
                                let w = (ui.available_width() - 560.0).max(0.0) / 2.0;
                                ui.add_space(w);
                                ui.vertical(|ui| {
                                    ui.set_width(560.0f32.min(ui.available_width()));
                                    ui.label(egui::RichText::new(texto).size(self.font_size).line_height(Some(self.font_size * 1.6)));
                                });
                            });
                        } else {
                            ui.label("Nenhuma pagina.");
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

    #[test]
    fn tema_config_roundtrip() {
        for t in Theme::ALL {
            let cfg = Config { theme: t };
            let json = serde_json::to_string(&cfg).expect("serializa");
            let back: Config = serde_json::from_str(&json).expect("desserializa");
            assert_eq!(back.theme, t);
        }
        // config ausente/corrompida cai no padrao sem quebrar
        let bad: Config = serde_json::from_str("{invalido}").unwrap_or_default();
        assert_eq!(bad.theme, Theme::Sepia);
    }
    #[test]
    fn pastas_drop_e_criacao() {
        let mut cols: Vec<Collection> = Vec::new();
        // drop de A sobre B (avulso) -> UI abriria modal; aqui simula criacao confirmada
        remove_from_all(&mut cols, "A");
        remove_from_all(&mut cols, "B");
        cols.push(Collection { name: "Mangas".to_string(), books: vec!["B".to_string(), "A".to_string()], collapsed: false });
        assert_eq!(cols.len(), 1);
        // drop de C sobre A (que esta na pasta 0) -> entra na mesma pasta
        let idx = cols.iter().position(|c| c.books.iter().any(|b| b == "A")).unwrap();
        remove_from_all(&mut cols, "C");
        cols[idx].books.push("C".to_string());
        assert_eq!(cols[0].books.len(), 3);
        // tirar B: pasta continua com 2
        remove_from_all(&mut cols, "B");
        assert_eq!(cols[0].books, vec!["A".to_string(), "C".to_string()]);
        // tirar A: pasta dissolve (< 2)
        remove_from_all(&mut cols, "A");
        assert!(cols.is_empty());
        // A e B na mesma pasta: drop entre eles nao muda nada (a UI nem abre modal)
        cols.push(Collection { name: "Y".to_string(), books: vec!["A".to_string(), "B".to_string()], collapsed: false });
        let ia = cols.iter().position(|c| c.books.iter().any(|b| b == "A"));
        let ib = cols.iter().position(|c| c.books.iter().any(|b| b == "B"));
        assert_eq!(ia, ib);
        assert_eq!(cols[ia.unwrap()].books.len(), 2);
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
        let imgs: Vec<_> = pages.iter().filter_map(|p| match p {
            Page::Image { bytes, w, h } => Some((bytes, w, h)),
            _ => None,
        }).collect();
        assert!(imgs.len() >= 150, "esperava 150+ paginas de imagem, achou {} em {} pags", imgs.len(), pages.len());
        // garante que a UI consegue decodificar (mesmo caminho do page_texture)
        for (bytes, w, h) in imgs.iter().take(5) {
            let img = image::load_from_memory(bytes).expect("jpeg deve decodificar");
            let rgba = img.to_rgba8();
            assert_eq!((rgba.width(), rgba.height()), (**w, **h));
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 750.0]),
        ..Default::default()
    };
    eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
