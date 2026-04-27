use anyhow::Result;
use clap::Parser;

use corky::cli::{
    CalCommands, ChatCommands, Cli, Commands, ContactCommands, DocCommands, DocsCommands,
    DraftCommands, FilterCommands, GscCommands, GscOutputFormat, LabelCommands, LinkedinCommands,
    MailboxCommands, PlaylistCommands, RagieCommands, ScheduleCommands, SheetsCommands,
    SiftCommands, SkillCommands, SlackCommands, SyncCommands, TasksCommands, TopicCommands,
    YoutubeCommands,
};

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle --mailbox: resolve named mailbox and set CORKY_DATA
    if let Some(ref mailbox_name) = cli.mailbox {
        let path = corky::app_config::resolve_mailbox(Some(mailbox_name))?;
        if let Some(p) = path {
            // SAFETY: This runs at the very start of main before any threads are spawned.
            unsafe { std::env::set_var("CORKY_DATA", p.to_string_lossy().as_ref()) };
        } else {
            eprintln!("No mailboxes configured. Run 'corky init' first.");
            std::process::exit(1);
        }
    }

    // Warn about available upgrades (skip if running the upgrade command itself)
    if !matches!(cli.command, Commands::Upgrade) {
        corky::upgrade::warn_if_outdated();
    }

    match cli.command {
        Commands::Init {
            path,
            user,
            provider,
            password_cmd,
            labels,
            github_user,
            name,
            sync,
            mailbox_name,
            force,
        } => corky::init::run(
            &user,
            &path,
            &provider,
            &password_cmd,
            &labels,
            &github_user,
            &name,
            sync,
            &mailbox_name,
            force,
        ),
        Commands::Sync { command } => match command {
            None => corky::sync::run(false, None),
            Some(SyncCommands::Full) => corky::sync::run(true, None),
            Some(SyncCommands::Account { name }) => corky::sync::run(false, Some(&name)),
            Some(SyncCommands::Routes) => corky::sync::routes::run(),
            Some(SyncCommands::Mailbox { name }) => corky::mailbox::sync::run(name.as_deref()),
            Some(SyncCommands::TelegramImport {
                path,
                label,
                account,
            }) => {
                let out_dir = corky::resolve::conversations_dir();
                corky::sync::telegram_import::run(&path, &label, &out_dir, &account)
            }
            Some(SyncCommands::SmsImport {
                path,
                label,
                account,
            }) => {
                let out_dir = corky::resolve::conversations_dir();
                corky::sync::sms_import::run(&path, &label, &out_dir, &account)
            }
            Some(SyncCommands::Imports) => corky::sync::imports::run_from_config(),
            Some(SyncCommands::Refetch { thread_id, json }) => {
                if json {
                    let report = corky::sync::refetch_report(&thread_id)?;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                    Ok(())
                } else {
                    corky::sync::refetch(&thread_id)
                }
            }
        },
        Commands::SyncAuth => corky::sync::auth::run(),
        Commands::ListFolders { account } => corky::sync::folders::run(account.as_deref()),
        Commands::PushDraft { file, send } => corky::draft::run(&file, send),
        Commands::AddLabel { label, account } => corky::accounts::add_label_cmd(&label, &account),
        Commands::Contact(cmd) => match cmd {
            ContactCommands::Add { name, emails, from } => {
                if let Some(slug) = from {
                    corky::contact::from_conversation::run(&slug, name.as_deref())
                } else {
                    let name =
                        name.ok_or_else(|| anyhow::anyhow!("NAME required when not using --from"))?;
                    corky::contact::add::run(&name, &emails)
                }
            }
            ContactCommands::Info { name } => corky::contact::info::run(&name),
            ContactCommands::Push { platform, names } => match platform.as_deref() {
                None | Some("google") => corky::contact::push::run_google(&names),
                Some(p) => anyhow::bail!("Unknown platform: {}. Supported: google", p),
            },
            ContactCommands::Delete { resource_names } => {
                corky::contact::delete::run(&resource_names)
            }
            ContactCommands::Sync => corky::contact::sync::run(),
        },
        Commands::ContactAdd {
            name,
            emails,
            labels: _,
            account: _,
        } => corky::contact::add::run(&name, &emails),
        Commands::Watch { interval } => corky::watch::run(interval),
        Commands::InstallSkill { name } => corky::skill::run(&name),
        Commands::Skill(cmd) => match cmd {
            SkillCommands::Install => corky::skill::install(),
            SkillCommands::Check => corky::skill::check(),
        },
        Commands::AuditDocs => corky::audit_docs::run(),
        Commands::Help { filter } => corky::help::run(filter.as_deref()),
        Commands::Unanswered { scope, from_name } => {
            let from = resolve_from_name(from_name)?;
            let scope = corky::mailbox::find_unanswered::Scope::from_arg(scope.as_deref());
            corky::mailbox::find_unanswered::run(scope, &from)
        }
        Commands::ValidateDraft { files } => corky::mailbox::validate_draft::run(&files),
        Commands::Draft(cmd) => run_draft_command(cmd),
        Commands::Mailbox(cmd) => match cmd {
            MailboxCommands::List => corky::mailbox::list::run(),
            MailboxCommands::Add {
                name,
                labels,
                display_name,
                github,
                github_user,
                pat,
                public,
                account,
                org,
            } => corky::mailbox::add::run(
                &name,
                &labels,
                &display_name,
                github,
                &github_user,
                pat,
                public,
                &account,
                &org,
            ),
            MailboxCommands::Sync { name } => corky::mailbox::sync::run(name.as_deref()),
            MailboxCommands::Status => corky::mailbox::sync::status(),
            MailboxCommands::Remove { name, delete_repo } => {
                corky::mailbox::remove::run(&name, delete_repo)
            }
            MailboxCommands::Rename {
                old_name,
                new_name,
                rename_repo,
            } => corky::mailbox::rename::run(&old_name, &new_name, rename_repo),
            MailboxCommands::Reset { name, no_sync } => {
                corky::mailbox::reset::run(name.as_deref(), no_sync)
            }
            MailboxCommands::Unanswered { scope, from_name } => {
                let from = resolve_from_name(from_name)?;
                let scope = corky::mailbox::find_unanswered::Scope::from_arg(scope.as_deref());
                corky::mailbox::find_unanswered::run(scope, &from)
            }
            MailboxCommands::Draft(cmd) => run_draft_command(cmd),
        },
        Commands::Linkedin(cmd) => match cmd {
            LinkedinCommands::Auth { profile } => {
                corky::social::run_auth("linkedin", profile.as_deref())
            }
            LinkedinCommands::Draft {
                body,
                author,
                visibility,
                tags,
            } => corky::social::run_draft(
                "linkedin",
                body.as_deref(),
                author.as_deref(),
                &visibility,
                &tags,
            ),
            LinkedinCommands::Publish { file, dry_run } => {
                corky::social::run_publish(&file, dry_run)
            }
            LinkedinCommands::Edit { file, body } => {
                corky::social::run_edit(&file, body.as_deref())
            }
            LinkedinCommands::Check => corky::social::run_check(),
            LinkedinCommands::List { status } => corky::social::run_list(status.as_deref()),
            LinkedinCommands::Comment { file, body } => corky::social::run_comment(&file, &body),
            LinkedinCommands::RenameAuthor { old, new } => {
                corky::social::run_rename_author(&old, &new)
            }
        },
        Commands::Youtube(cmd) => match cmd {
            YoutubeCommands::Auth { profile } => {
                corky::social::run_auth("youtube", profile.as_deref())
            }
            YoutubeCommands::Draft {
                body,
                author,
                visibility,
                tags,
            } => corky::social::run_draft(
                "youtube",
                body.as_deref(),
                author.as_deref(),
                &visibility,
                &tags,
            ),
            YoutubeCommands::Publish { file, dry_run } => {
                corky::social::run_publish(&file, dry_run)
            }
            YoutubeCommands::Delete { video_id } => corky::social::run_youtube_delete(&video_id),
            YoutubeCommands::Edit { file } => corky::social::run_youtube_edit(&file),
            YoutubeCommands::Comment { file, body } => {
                corky::social::run_youtube_comment(&file, &body)
            }
            YoutubeCommands::Playlist(cmd) => match cmd {
                PlaylistCommands::Create {
                    title,
                    description,
                    visibility,
                } => corky::social::run_playlist_create(&title, &description, &visibility),
                PlaylistCommands::Add {
                    playlist_id,
                    video_id,
                    position,
                } => corky::social::run_playlist_add(&playlist_id, &video_id, position),
                PlaylistCommands::List => corky::social::run_playlist_list(),
                PlaylistCommands::Remove {
                    playlist_id,
                    video_id,
                } => corky::social::run_playlist_remove(&playlist_id, &video_id),
            },
            YoutubeCommands::Check => corky::social::run_check(),
            YoutubeCommands::List { status } => corky::social::run_list(status.as_deref()),
        },
        Commands::Schedule(cmd) => match cmd {
            ScheduleCommands::Run { dry_run } => corky::schedule::run(dry_run),
            ScheduleCommands::List => corky::schedule::list(),
        },
        Commands::Topics(cmd) => match cmd {
            TopicCommands::List { verbose } => corky::topics::run_list(verbose),
            TopicCommands::Add {
                name,
                keywords,
                description,
            } => corky::topics::run_add(&name, &keywords, description.as_deref()),
            TopicCommands::Info { name } => corky::topics::run_info(&name),
            TopicCommands::Suggest { limit, mailbox } => {
                corky::topics::run_suggest(limit, mailbox.as_deref())
            }
        },
        Commands::Slack(cmd) => match cmd {
            SlackCommands::Import {
                path,
                label,
                account,
            } => {
                let out_dir = corky::resolve::conversations_dir();
                corky::sync::slack_import::run(&path, &label, &out_dir, &account)
            }
        },
        Commands::Label(cmd) => match cmd {
            LabelCommands::Clear {
                label,
                account,
                search,
                dry_run,
            } => corky::label::clear::run(&label, account.as_deref(), search.as_deref(), dry_run),
        },
        Commands::Cal(cmd) => match cmd {
            CalCommands::Auth { account } => corky::cal::auth::run_auth(account.as_deref()),
            CalCommands::List {
                limit,
                query,
                account,
            } => corky::cal::list::run(limit, query.as_deref(), account.as_deref()),
            CalCommands::Delete {
                query,
                all,
                dry_run,
                account,
            } => corky::cal::delete::run(&query, all, dry_run, account.as_deref()),
            CalCommands::Create {
                summary,
                start,
                end,
                description,
                location,
                account,
            } => corky::cal::create::run(
                &summary,
                &start,
                &end,
                description.as_deref(),
                location.as_deref(),
                account.as_deref(),
            ),
            CalCommands::Check {
                start,
                end,
                account,
            } => corky::cal::check::run(&start, &end, account.as_deref()),
        },
        Commands::Gsc(cmd) => run_gsc(cmd),
        Commands::Filter(cmd) => match cmd {
            FilterCommands::Build { input, output } => {
                corky::filter::build::run(input.as_deref(), output.as_deref())
            }
            FilterCommands::Auth { account } => {
                corky::filter::gmail_auth::run_auth(account.as_deref())
            }
            FilterCommands::Pull { account } => corky::filter::pull::run(account.as_deref()),
            FilterCommands::Push { account, dry_run } => {
                corky::filter::push::run(account.as_deref(), dry_run)
            }
            FilterCommands::Check { account } => {
                corky::filter::check::run(account.as_deref())?;
                Ok(())
            }
        },
        Commands::Doc(cmd) => match cmd {
            DocCommands::Build {
                file,
                format,
                template,
                output,
            } => corky::doc::build::run(&file, &format, template.as_deref(), output.as_deref()),
            DocCommands::Upload {
                file,
                share,
                account,
            } => {
                let link = corky::doc::upload::run(&file, share, account.as_deref())?;
                println!("{}", link);
                Ok(())
            }
            DocCommands::Read {
                doc,
                output,
                account,
            } => corky::doc::gdocs::read(&doc, output.as_deref(), account.as_deref()),
            DocCommands::Write { doc, file, account } => {
                corky::doc::gdocs::write(&doc, &file, account.as_deref())
            }
            DocCommands::Sheet {
                sheet,
                range,
                format,
                output,
                account,
            } => corky::doc::sheets::read(
                &sheet,
                range.as_deref(),
                &format,
                output.as_deref(),
                account.as_deref(),
            ),
            DocCommands::SheetWrite {
                sheet,
                range,
                file,
                account,
            } => corky::doc::sheets::write(&sheet, &range, &file, account.as_deref()),
        },
        Commands::Docs(cmd) => match cmd {
            DocsCommands::Read {
                doc,
                output,
                account,
            } => corky::doc::gdocs::read(&doc, output.as_deref(), account.as_deref()),
            DocsCommands::Write { doc, file, account } => {
                corky::doc::gdocs::write(&doc, &file, account.as_deref())
            }
        },
        Commands::Sheets(cmd) => match cmd {
            SheetsCommands::Read {
                sheet,
                range,
                format,
                output,
                account,
            } => corky::doc::sheets::read(
                &sheet,
                range.as_deref(),
                &format,
                output.as_deref(),
                account.as_deref(),
            ),
            SheetsCommands::Write {
                sheet,
                range,
                file,
                account,
            } => corky::doc::sheets::write(&sheet, &range, &file, account.as_deref()),
        },
        Commands::Transcribe {
            file,
            model,
            language,
            output,
            speakers,
            diarize,
            no_adaptive_chunk,
            no_resolve_unknown,
            no_confidence_retranscribe,
        } => corky::transcribe::run(
            &file,
            model.as_deref(),
            language.as_deref(),
            output.as_deref(),
            &speakers,
            diarize,
            no_adaptive_chunk,
            no_resolve_unknown,
            no_confidence_retranscribe,
        ),
        Commands::Search {
            query,
            backend,
            all,
        } => corky::search::run(&query, backend.as_deref(), all),
        Commands::Sift(cmd) => match cmd {
            SiftCommands::Index { watch } => corky::search::sift::SiftBackend::run_index(watch),
            SiftCommands::Status => corky::search::sift::SiftBackend::run_status(),
        },
        Commands::Ragie(cmd) => match cmd {
            RagieCommands::Push { full } => corky::search::ragie::RagieBackend::run_push(full),
            RagieCommands::Sync => corky::search::ragie::RagieBackend::run_sync(),
            RagieCommands::Search { query } => {
                // Direct Ragie search — use the unified search with ragie backend
                corky::search::run(&query, Some("ragie"), false)
            }
            RagieCommands::Status => corky::search::ragie::RagieBackend::run_status(),
        },
        Commands::Chat(cmd) => match cmd {
            ChatCommands::Send {
                space,
                message,
                account,
            } => corky::social::chat::send(&space, &message, account.as_deref()),
        },
        Commands::Tasks(cmd) => match cmd {
            TasksCommands::List { tasklist, account } => {
                corky::tasks::list::run(tasklist.as_deref(), account.as_deref())
            }
            TasksCommands::Add {
                title,
                due,
                tasklist,
                account,
            } => corky::tasks::add::run(
                &title,
                due.as_deref(),
                tasklist.as_deref(),
                account.as_deref(),
            ),
            TasksCommands::Done {
                task_id,
                tasklist,
                account,
            } => corky::tasks::done::run(&task_id, tasklist.as_deref(), account.as_deref()),
        },
        Commands::Doctor { provider, json } => corky::doctor::run(provider.as_deref(), json),
        Commands::Upgrade => corky::upgrade::run(),
    }
}

fn run_draft_command(cmd: DraftCommands) -> anyhow::Result<()> {
    match cmd {
        DraftCommands::New {
            subject,
            to,
            cc,
            account,
            from,
            in_reply_to,
            thread_id,
            mailbox,
            attachments,
            images,
        } => corky::draft::new::run(
            &subject,
            &to,
            cc.as_deref(),
            account.as_deref(),
            from.as_deref(),
            in_reply_to.as_deref(),
            thread_id.as_deref(),
            mailbox.as_deref(),
            &attachments,
            &images,
        ),
        DraftCommands::Attach {
            file,
            files,
            clipboard,
            inline,
        } => corky::draft::attach::run(&file, &files, clipboard, inline),
        DraftCommands::Validate { args } => corky::mailbox::validate_draft::run_scoped(&args),
        DraftCommands::Push { file, send, json } => {
            if json {
                let report = corky::draft::run_with_report(&file, send)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                corky::draft::run(&file, send)
            }
        }
        DraftCommands::Send {
            file,
            attachments,
            account,
            json,
        } => {
            if json {
                let report =
                    corky::draft::send::run_with_report(&file, &attachments, account.as_deref())?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                corky::draft::send::run(&file, &attachments, account.as_deref())
            }
        }
        DraftCommands::Migrate { dry_run } => corky::draft::migrate::run(dry_run),
    }
}

fn run_gsc(cmd: GscCommands) -> Result<()> {
    match cmd {
        GscCommands::Auth { account } => corky::gsc::auth::run_auth(account.as_deref()),
        GscCommands::Sites { format } => {
            let token = corky::gsc::auth::get_access_token(None)?;
            let sites = corky::gsc::sites::list_sites(&token)?;
            print_gsc_sites(&sites, format)
        }
        GscCommands::Query {
            site,
            start,
            end,
            dimensions,
            row_limit,
            format,
        } => {
            let token = corky::gsc::auth::get_access_token(None)?;
            let dims: Vec<&str> = dimensions.iter().map(String::as_str).collect();
            let params = corky::gsc::query::QueryParams {
                site_url: &site,
                start_date: &start,
                end_date: &end,
                dimensions: &dims,
                row_limit,
                start_row: 0,
                filters: vec![],
            };
            let resp = corky::gsc::query::run_query(&token, &params)?;
            print_gsc_query(&resp, format)
        }
        GscCommands::Inspect { site, url } => {
            let token = corky::gsc::auth::get_access_token(None)?;
            let resp = corky::gsc::inspect::inspect_url(&token, &site, &url)?;
            println!("{}", serde_json::to_string_pretty(&resp)?);
            Ok(())
        }
    }
}

fn print_gsc_sites(sites: &[corky::gsc::sites::SiteEntry], format: GscOutputFormat) -> Result<()> {
    match format {
        GscOutputFormat::Json => {
            let v: Vec<serde_json::Value> = sites
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "siteUrl": s.site_url,
                        "permissionLevel": s.permission_level,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        GscOutputFormat::Csv => {
            println!("site_url,permission_level");
            for s in sites {
                println!("{},{}", s.site_url, s.permission_level);
            }
        }
        GscOutputFormat::Table => {
            if sites.is_empty() {
                println!(
                    "(no sites — ensure the SA email is added in Search Console → Settings → Users & permissions)"
                );
            }
            let url_w = sites
                .iter()
                .map(|s| s.site_url.len())
                .max()
                .unwrap_or(8)
                .max(8);
            println!("{:<width$}  permission", "site_url", width = url_w);
            for s in sites {
                println!(
                    "{:<width$}  {}",
                    s.site_url,
                    s.permission_level,
                    width = url_w
                );
            }
        }
    }
    Ok(())
}

fn print_gsc_query(resp: &corky::gsc::query::QueryResponse, format: GscOutputFormat) -> Result<()> {
    match format {
        GscOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(resp)?);
        }
        GscOutputFormat::Csv => {
            let rows = resp.rows.as_deref().unwrap_or(&[]);
            println!("keys,clicks,impressions,ctr,position");
            for r in rows {
                println!(
                    "{},{},{},{},{}",
                    r.keys.join("|"),
                    r.clicks,
                    r.impressions,
                    r.ctr,
                    r.position
                );
            }
        }
        GscOutputFormat::Table => {
            let rows = resp.rows.as_deref().unwrap_or(&[]);
            println!(
                "{:>9} {:>12} {:>8} {:>8}  keys",
                "clicks", "impressions", "ctr", "pos"
            );
            for r in rows {
                println!(
                    "{:>9.0} {:>12.0} {:>8.4} {:>8.2}  {}",
                    r.clicks,
                    r.impressions,
                    r.ctr,
                    r.position,
                    r.keys.join(" | ")
                );
            }
        }
    }
    Ok(())
}

/// Resolve the --from name: CLI flag > owner.name in .corky.toml > error.
fn resolve_from_name(from_name: Option<String>) -> anyhow::Result<String> {
    if let Some(name) = from_name {
        return Ok(name);
    }
    if let Some(cfg) = corky::config::corky_config::try_load_config(None)
        && let Some(owner) = cfg.owner
        && !owner.name.is_empty()
    {
        return Ok(owner.name);
    }
    anyhow::bail!(
        "No --from name provided and no [owner] name in .corky.toml.\n\
         Use --from NAME or set name in [owner] section of .corky.toml."
    )
}
