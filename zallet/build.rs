use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Command, CommandFactory, ValueEnum};
use clap_complete::{Shell, generate_to};
use clap_mangen::Man;
use flate2::{Compression, write::GzEncoder};
use i18n_embed::unic_langid::LanguageIdentifier;

const JSON_RPC_METHODS_RS: &str = "src/components/json_rpc/methods.rs";

mod i18n {
    include!("src/i18n.rs");
}
mod zallet {
    include!("src/cli.rs");
}

#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id)
    }};

    ($message_id:literal, $($args:expr),* $(,)?) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id, $($args), *)
    }};
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/cli.rs");
    println!("cargo::rerun-if-changed=src/i18n.rs");
    println!("cargo::rerun-if-changed={JSON_RPC_METHODS_RS}");

    // Expose a cfg option so we can make parts of the CLI conditional on not being built
    // within the buildscript.
    println!("cargo:rustc-cfg=outside_buildscript");

    let out_dir = match env::var_os("OUT_DIR") {
        None => return Ok(()),
        Some(out_dir) => PathBuf::from(out_dir),
    };

    // `OUT_DIR` is "intentionally opaque as it is only intended for `rustc` interaction"
    // (https://github.com/rust-lang/cargo/issues/9858). Peek into the black box and use
    // it to figure out where the target directory is.
    let target_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("should be absolute path")
        .to_path_buf();

    generate_rpc_help(&out_dir)?;

    // Generate the completions in English, because these aren't easily localizable.
    i18n::load_languages(&[]);
    Cli::build().generate_completions(&target_dir.join("completions"))?;

    // Generate manpages for all supported languages.
    let manpage_dir = target_dir.join("manpages");
    for lang_dir in fs::read_dir("./i18n")? {
        let lang_dir = lang_dir?.file_name();
        let lang_dir = lang_dir.to_str().expect("should be valid Unicode");
        println!("cargo::rerun-if-changed=i18n/{lang_dir}/zallet.ftl");

        let lang: LanguageIdentifier = lang_dir
            .parse()
            .expect("should be valid language identifier");

        // Render the manpages into the correct folder structure, so that local checks can
        // be performed with `man -M target/debug/manpages BINARY_NAME`.
        let mut out_dir = if lang.language.as_str() == "en" {
            manpage_dir.clone()
        } else {
            let mut lang_str = lang.language.as_str().to_owned();
            if let Some(region) = lang.region {
                // Locales for manpages use the POSIX format with underscores.
                lang_str += "_";
                lang_str += region.as_str();
            }
            manpage_dir.join(lang_str)
        };
        out_dir.push("man1");

        i18n::load_languages(&[lang]);
        Cli::build().generate_manpages(&out_dir)?;
    }

    Ok(())
}

#[derive(Clone)]
struct Cli {
    zallet: Command,
}

impl Cli {
    fn build() -> Self {
        Self {
            zallet: zallet::EntryPoint::command(),
        }
    }

    fn generate_completions(&mut self, out_dir: &Path) -> io::Result<()> {
        fs::create_dir_all(out_dir)?;

        for &shell in Shell::value_variants() {
            generate_to(shell, &mut self.zallet, "zallet", out_dir)?;
        }

        Ok(())
    }

    fn generate_manpages(self, out_dir: &Path) -> io::Result<()> {
        fs::create_dir_all(out_dir)?;

        fn generate_manpage(
            out_dir: &Path,
            name: &str,
            cmd: Command,
            custom: impl FnOnce(&Man, &mut GzEncoder<fs::File>) -> io::Result<()>,
        ) -> io::Result<()> {
            let file = fs::File::create(out_dir.join(format!("{}.1.gz", name)))?;
            let mut w = GzEncoder::new(file, Compression::best());

            let man = Man::new(cmd);
            man.render_title(&mut w)?;
            man.render_name_section(&mut w)?;
            man.render_synopsis_section(&mut w)?;
            man.render_description_section(&mut w)?;
            man.render_options_section(&mut w)?;
            custom(&man, &mut w)?;
            man.render_version_section(&mut w)?;
            man.render_authors_section(&mut w)
        }

        generate_manpage(
            out_dir,
            "zallet",
            self.zallet
                .about(fl!("man-zallet-about"))
                .long_about(fl!("man-zallet-description")),
            |_, _| Ok(()),
        )?;

        Ok(())
    }
}

fn generate_rpc_help(out_dir: &Path) -> Result<(), Box<dyn Error>> {
    // Parse the source file containing the `Rpc` trait.
    let methods_rs = fs::read_to_string(JSON_RPC_METHODS_RS)?;
    let methods_ast = syn::parse_file(&methods_rs)?;

    let rpc_trait = methods_ast
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Trait(item_trait) if item_trait.ident == "Rpc" => Some(item_trait),
            _ => None,
        })
        .expect("present");

    let mut contents = "static COMMANDS: ::phf::Map<&str, &str> = ::phf::phf_map! {\n".to_string();

    for item in &rpc_trait.items {
        match item {
            syn::TraitItem::Fn(method) => {
                // Find methods via their `#[method(name = "command")]` attribute.
                let mut command = None;
                method
                    .attrs
                    .iter()
                    .find(|attr| attr.path().is_ident("method"))
                    .and_then(|attr| {
                        attr.parse_nested_meta(|meta| {
                            command = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                            Ok(())
                        })
                        .ok()
                    });

                if let Some(command) = command {
                    contents.push('"');
                    contents.push_str(&command);
                    contents.push_str("\" => \"");

                    for attr in method
                        .attrs
                        .iter()
                        .filter(|attr| attr.path().is_ident("doc"))
                    {
                        if let syn::Meta::NameValue(doc_line) = &attr.meta {
                            if let syn::Expr::Lit(docs) = &doc_line.value {
                                if let syn::Lit::Str(s) = &docs.lit {
                                    // Trim the leading space from the doc comment line.
                                    let line = s.value();
                                    let trimmed_line =
                                        if line.is_empty() { &line } else { &line[1..] };

                                    let escaped = trimmed_line.escape_default().collect::<String>();

                                    contents.push_str(&escaped);
                                    contents.push_str("\\n");
                                }
                            }
                        }
                    }

                    contents.push_str("\",\n");
                }
            }
            _ => (),
        }
    }

    contents.push_str("};");

    let rpc_help_path = out_dir.join("rpc_help.rs");
    fs::write(&rpc_help_path, contents)?;

    Ok(())
}
