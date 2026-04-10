use crate::cli::models::ModelsCommand;
use crate::models::{
    capabilities, catalog, download_exact_ref, find_catalog_model_exact, huggingface_hub_cache_dir,
    installed_model_capabilities, scan_installed_models, search_catalog_models, search_huggingface,
    show_exact_model, show_model_variants, SearchArtifactFilter, SearchProgress,
};
use crate::system::hardware;
use anyhow::{anyhow, Result};
use std::io::Write;
use std::path::Path;

pub async fn run_model_search(
    query: &[String],
    prefer_gguf: bool,
    prefer_mlx: bool,
    catalog_only: bool,
    limit: usize,
) -> Result<()> {
    let query = query.join(" ");
    let filter = if prefer_mlx {
        SearchArtifactFilter::Mlx
    } else if prefer_gguf {
        SearchArtifactFilter::Gguf
    } else {
        SearchArtifactFilter::Gguf
    };
    let filter_label = match filter {
        SearchArtifactFilter::Gguf => "GGUF",
        SearchArtifactFilter::Mlx => "MLX",
    };

    if catalog_only {
        let results: Vec<_> = search_catalog_models(&query)
            .into_iter()
            .filter(|model| match filter {
                SearchArtifactFilter::Gguf => !catalog_model_is_mlx(model),
                SearchArtifactFilter::Mlx => catalog_model_is_mlx(model),
            })
            .collect();
        if results.is_empty() {
            eprintln!("🔎 No {filter_label} catalog models matched '{query}'.");
            return Ok(());
        }
        println!("📚 {filter_label} catalog matches for '{query}'");
        if let Some(summary) = local_capacity_summary() {
            println!("{}", summary);
        }
        println!();
        for model in results.into_iter().take(limit) {
            println!("• {}  {}", model.name, model.size);
            println!("  {}", model.description);
            if let Some(fit) = fit_hint_for_size_label(&model.size) {
                println!("  {}", fit);
            }
        }
        return Ok(());
    }

    eprintln!("🔎 Searching Hugging Face {filter_label} repos for '{query}'...");
    let mut announced_repo_scan = false;
    let results = search_huggingface(&query, limit, filter, |progress| match progress {
        SearchProgress::SearchingHub => {}
        SearchProgress::InspectingRepos { completed, total } => {
            if total == 0 {
                return;
            }
            if !announced_repo_scan {
                announced_repo_scan = true;
                eprintln!("   Inspecting {total} candidate repos...");
            }
            if completed == 0 {
                return;
            }
            eprint!("\r   Inspected {completed}/{total} candidate repos...");
            let _ = std::io::stderr().flush();
            if completed == total {
                eprintln!();
            }
        }
    })
    .await?;
    if results.is_empty() {
        eprintln!("🔎 No Hugging Face {filter_label} matches for '{query}'.");
        return Ok(());
    }

    println!("🔎 Hugging Face {filter_label} matches for '{query}'");
    if let Some(summary) = local_capacity_summary() {
        println!("{}", summary);
    }
    println!();
    for (index, result) in results.iter().enumerate() {
        println!("{}. 📦 {}", index + 1, result.repo_id);
        println!("   type: {}", result.kind);
        let mut stats = Vec::new();
        if let Some(size) = &result.size_label {
            stats.push(format!("📏 {}", size));
        }
        if let Some(downloads) = result.downloads {
            stats.push(format!("⬇️ {}", format_count(downloads)));
        }
        if let Some(likes) = result.likes {
            stats.push(format!("❤️ {}", format_count(likes)));
        }
        if !stats.is_empty() {
            println!("   {}", stats.join("  "));
        }
        let mut caps = vec!["💬 text".to_string()];
        if result.capabilities.multimodal_label().is_some() {
            caps.push("🎛️ multimodal".to_string());
        }
        if let Some(label) = result.capabilities.vision_label() {
            caps.push(format!("👁️ vision ({label})"));
        }
        if let Some(label) = result.capabilities.audio_label() {
            caps.push(format!("🔊 audio ({label})"));
        }
        if let Some(label) = result.capabilities.reasoning_label() {
            caps.push(format!("🧠 reasoning ({label})"));
        }
        if let Some(label) = result.capabilities.tool_use_label() {
            caps.push(format!("🛠️ tool use ({label})"));
        }
        println!("   capabilities: {}", caps.join("  "));
        println!("   ref: {}", result.exact_ref);
        println!("   show: mesh-llm models show {}", result.exact_ref);
        println!("   download: mesh-llm models download {}", result.exact_ref);
        if let Some(size) = &result.size_label {
            if let Some(fit) = fit_hint_for_size_label(size) {
                println!("   {}", fit);
            }
        }
        if let Some(model) = result.catalog {
            println!("   ⭐ Recommended: {} ({})", model.name, model.size);
            println!("   {}", model.description);
        }
        println!();
    }
    Ok(())
}

pub fn run_model_recommended() {
    println!("📚 Recommended models");
    println!();
    for model in catalog::MODEL_CATALOG.iter() {
        let model_capabilities = capabilities::infer_catalog_capabilities(model);
        println!("• {}  {}", model.name, model.size);
        println!("  {}", model.description);
        if let Some(draft) = model.draft.as_deref() {
            println!("  🧠 Draft: {}", draft);
        }
        if let Some(label) = model_capabilities.vision_label() {
            println!("  👁️ Vision: {}", label);
        }
        if let Some(label) = model_capabilities.audio_label() {
            println!("  🔊 Audio: {}", label);
        }
        if let Some(label) = model_capabilities.reasoning_label() {
            println!("  🧠 Reasoning: {}", label);
        }
        if model.moe.is_some() {
            println!("  🧩 MoE: yes");
        }
        println!();
    }
}

pub fn run_model_installed() {
    let installed = scan_installed_models();
    if installed.is_empty() {
        println!("📦 No installed models found");
        println!("   HF cache: {}", huggingface_hub_cache_dir().display());
        return;
    }

    println!("💾 Installed models");
    println!("📁 HF cache: {}", huggingface_hub_cache_dir().display());
    println!();
    for name in installed.iter() {
        let path = crate::models::find_model_path(&name);
        let size = std::fs::metadata(&path).map(|meta| meta.len()).ok();
        let catalog_model = find_catalog_model_exact(&name);
        let model_capabilities = installed_model_capabilities(&name);

        println!("📦 {}", name);
        println!("   type: {}", installed_model_kind(&path));
        if let Some(bytes) = size {
            println!("   📏 {}", format_installed_size(bytes));
        }
        let mut caps = vec!["💬 text".to_string()];
        if model_capabilities.multimodal_label().is_some() {
            caps.push("🎛️ multimodal".to_string());
        }
        if let Some(label) = model_capabilities.vision_label() {
            caps.push(format!("👁️ vision ({label})"));
        }
        if let Some(label) = model_capabilities.audio_label() {
            caps.push(format!("🔊 audio ({label})"));
        }
        if let Some(label) = model_capabilities.reasoning_label() {
            caps.push(format!("🧠 reasoning ({label})"));
        }
        if let Some(label) = model_capabilities.tool_use_label() {
            caps.push(format!("🛠️ tool use ({label})"));
        }
        println!("   capabilities: {}", caps.join("  "));
        println!("   ref: {}", name);
        println!("   show: mesh-llm models show {}", name);
        println!("   download: mesh-llm models download {}", name);
        println!("   path: {}", path.display());
        if let Some(model) = catalog_model {
            println!("   about: {}", model.description);
            if let Some(draft) = model.draft.as_deref() {
                println!("   🧠 draft: {}", draft);
            }
            if model.moe.is_some() {
                println!("   🧩 MoE: yes");
            }
        }
        println!();
    }
}

pub async fn run_model_show(model_ref: &str) -> Result<()> {
    let details = show_exact_model(model_ref).await?;
    println!("🔎 {}", details.display_name);
    if let Some(summary) = local_capacity_summary() {
        println!("{}", summary);
    }
    println!();
    println!("Ref: {}", details.exact_ref);
    println!("Type: {}", details.kind);
    println!("Source: {}", format_source_label(details.source));
    if let Some(size) = details.size_label {
        println!("Size: {size}");
        if let Some(fit) = fit_hint_for_size_label(&size) {
            println!("Fit: {}", fit);
        }
    }
    if let Some(description) = details.description {
        println!("About: {description}");
    }
    if let Some(draft) = details.draft {
        println!("🧠 Draft: {draft}");
    }
    println!("Capabilities:");
    println!("  💬 text");
    if details.capabilities.multimodal_label().is_some() {
        println!("  🎛️ multimodal");
    }
    if let Some(label) = details.capabilities.vision_label() {
        println!("  👁️ vision ({label})");
    }
    if let Some(label) = details.capabilities.audio_label() {
        println!("  🔊 audio ({label})");
    }
    if let Some(label) = details.capabilities.reasoning_label() {
        println!("  🧠 reasoning ({label})");
    }
    if let Some(moe) = details.moe {
        println!(
            "🧩 MoE: {} experts, top-{}, min per node {}{}",
            moe.n_expert,
            moe.n_expert_used,
            moe.min_experts_per_node,
            if moe.ranking.is_empty() {
                ", no embedded ranking".to_string()
            } else {
                format!(", ranking {}", moe.ranking.len())
            }
        );
    }
    println!("📥 Download:");
    println!("   {}", details.download_url);

    if let Some(variants) = show_model_variants(model_ref).await? {
        if !variants.is_empty() {
            println!();
            println!("Variants:");
            let mut rows = Vec::new();
            for variant in variants {
                let size = variant.size_label.as_deref().unwrap_or("-");
                let fit = variant
                    .size_label
                    .as_deref()
                    .and_then(fit_hint_for_size_label)
                    .unwrap_or_else(|| "-".to_string());
                let selected = variant.exact_ref == details.exact_ref;
                rows.push((
                    variant_selector_label(&variant.exact_ref),
                    size.to_string(),
                    fit,
                    variant.exact_ref,
                    selected,
                ));
            }
            let quant_width = rows
                .iter()
                .map(|(quant, _, _, _, _)| quant.len())
                .max()
                .unwrap_or(5)
                .max("quant".len());
            let size_width = rows
                .iter()
                .map(|(_, size, _, _, _)| size.len())
                .max()
                .unwrap_or(4)
                .max("size".len());
            let fit_width = rows
                .iter()
                .map(|(_, _, fit, _, _)| fit.len())
                .max()
                .unwrap_or(3)
                .max("fit".len());

            println!(
                "{:<quant_width$}  {:>size_width$}  {:<fit_width$}  ref",
                "quant", "size", "fit"
            );
            println!(
                "{:-<quant_width$}  {:-<size_width$}  {:-<fit_width$}  {:-<3}",
                "", "", "", ""
            );

            for (quant, size, fit, r#ref, selected) in rows {
                println!(
                    "{:<quant_width$}  {:>size_width$}  {:<fit_width$}  {}{}",
                    quant,
                    size,
                    fit,
                    r#ref,
                    if selected { "  ← selected" } else { "" }
                );
            }
        }
    }
    Ok(())
}

pub async fn run_model_download(model_ref: &str, include_draft: bool) -> Result<()> {
    let details = show_exact_model(model_ref).await.ok();
    let path = download_exact_ref(model_ref).await?;
    println!("✅ Downloaded model");
    if let Some(details) = &details {
        println!("   type: {}", details.kind);
    }
    println!("   {}", path.display());

    if !include_draft {
        return Ok(());
    }

    let Some(details) = details else {
        return Ok(());
    };
    let Some(draft) = details.draft else {
        eprintln!("⚠ No draft model available for {}", details.display_name);
        return Ok(());
    };
    let draft_model = find_catalog_model_exact(&draft)
        .ok_or_else(|| anyhow!("Draft model '{}' not found in catalog", draft))?;
    let draft_path = catalog::download_model(draft_model).await?;
    println!("🧠 Downloaded draft");
    println!("   {}", draft_path.display());
    Ok(())
}

fn format_installed_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1}GB", bytes as f64 / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.0}MB", bytes as f64 / 1e6)
    } else {
        format!("{}B", bytes)
    }
}

fn installed_model_kind(path: &Path) -> &'static str {
    let text = path.to_string_lossy().to_ascii_lowercase();
    if text.ends_with(".safetensors")
        || text.ends_with(".safetensors.index.json")
        || text.contains("model.safetensors")
    {
        "🍎 MLX"
    } else {
        "🦙 GGUF"
    }
}

fn format_count(value: u64) -> String {
    let text = value.to_string();
    let mut out = String::with_capacity(text.len() + text.len() / 3);
    for (index, ch) in text.chars().enumerate() {
        if index > 0 && (text.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn format_source_label(source: &str) -> &'static str {
    match source {
        "catalog" => "Catalog",
        "huggingface" => "Hugging Face",
        "url" => "Direct URL",
        _ => "Unknown",
    }
}

fn local_capacity_summary() -> Option<String> {
    let vram_gb = hardware::survey().vram_bytes as f64 / 1e9;
    if vram_gb <= 0.0 {
        None
    } else {
        Some(format!("🖥️ This machine: ~{vram_gb:.1}GB available"))
    }
}

fn fit_hint_for_size_label(size_label: &str) -> Option<String> {
    let model_gb = catalog::parse_size_gb(size_label);
    let vram_gb = hardware::survey().vram_bytes as f64 / 1e9;
    if model_gb <= 0.0 || vram_gb <= 0.0 {
        return None;
    }

    let hint = if model_gb <= vram_gb * 0.6 {
        "✅ likely comfortable here"
    } else if model_gb <= vram_gb * 0.9 {
        "⚠️ likely fits, but tight"
    } else if model_gb <= vram_gb * 1.1 {
        "🟡 may load, but expect tradeoffs"
    } else {
        "⛔ likely too large for local serve"
    };
    Some(hint.to_string())
}

fn variant_selector_label(exact_ref: &str) -> String {
    if let Some((_, selector)) = exact_ref.split_once(':') {
        return selector
            .split_once('@')
            .map(|(value, _)| value)
            .unwrap_or(selector)
            .to_string();
    }
    Path::new(exact_ref)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(exact_ref)
        .to_string()
}

pub async fn dispatch_models_command(command: &ModelsCommand) -> Result<()> {
    match command {
        ModelsCommand::Recommended | ModelsCommand::List => run_model_recommended(),
        ModelsCommand::Installed => run_model_installed(),
        ModelsCommand::Search {
            query,
            gguf,
            mlx,
            catalog,
            limit,
        } => run_model_search(query, *gguf, *mlx, *catalog, *limit).await?,
        ModelsCommand::Show { model } => run_model_show(model).await?,
        ModelsCommand::Download { model, draft } => run_model_download(model, *draft).await?,
        ModelsCommand::Updates { repo, all, check } => {
            crate::models::run_update(repo.as_deref(), *all, *check)?
        }
    }
    Ok(())
}

fn catalog_model_is_mlx(model: &catalog::CatalogModel) -> bool {
    model
        .source_file()
        .map(|file| {
            file.ends_with("model.safetensors") || file.ends_with("model.safetensors.index.json")
        })
        .unwrap_or(false)
        || model.url.contains("model.safetensors")
}
