# LinkedIn Post Runbook

## Drafting

1. Write the post body as plain text (LinkedIn does not render markdown)
2. **No external links in the post body** — LinkedIn suppresses reach on posts containing outbound URLs
3. Move all links (blog posts, repos, videos) to a **first comment** drafted alongside the post
4. End the post body with a hook or question to encourage engagement

## Structure

**Post body:**
- Hook/title line (stands alone, grabs attention)
- 2-3 short paragraphs (value proposition, key insight, personal result)
- CTA or question (engagement driver)

**First comment (posted immediately after):**
- All external links (blog post, repo, video)
- Brief context for each link if multiple

## Formatting

- No markdown bold (`**text**`) — shows as literal asterisks
- Use line breaks for readability
- Unicode bold is an option but can look spammy — use sparingly
- Emojis are acceptable for visual breaks but don't overdo it

## Posting

1. Convert the approved body into a ready `mail/social/*.md` LinkedIn draft with YAML frontmatter.
2. Run `corky linkedin post <file>` (alias for `publish`).
3. Add the first comment with links using `corky linkedin comment <published-file> "<comment>"`.
4. Verify both post and comment are visible.

## Timing

- **Best:** Tuesday-Thursday, 8-10 AM ET (peak developer feed)
- **Good:** Monday 9 AM ET
- **Avoid:** Weekends, holidays, Friday afternoon
- Exception: post early if a specific person (interviewer, contact) will check your profile before a meeting
