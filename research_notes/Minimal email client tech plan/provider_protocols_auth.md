# Provider protocols and authentication for a third-party desktop email client (Gmail/Workspace, Microsoft, iCloud), state as of 4 November 2026

## Google: IMAP+XOAUTH2 vs Gmail REST API, scopes, verification, CASA, limits

### Takeaway
IMAP does not avoid Google's review: `https://mail.google.com/` is restricted, and so is every Gmail scope a full client needs. The cheap route is a personal-use app under 100 users, published "In production" but unverified. Going commercial means restricted-scope verification. Whether a no-backend client also needs paid CASA is unclear, because Google's own pages disagree.

### Cited Findings

**Endpoints and session behaviour**
- IMAP is `imap.gmail.com:993` (implicit TLS). SMTP is `smtp.gmail.com:465` (implicit TLS) or `:587` (STARTTLS). All support SASL XOAUTH2 — [Google IMAP/SMTP](https://developers.google.com/workspace/gmail/imap/imap-smtp)
- IMAP sessions last up to about 24 hours, but OAuth-authenticated sessions last about as long as the access token, usually about 1 hour. Gmail then closes the connection and the client must reconnect and re-authenticate — [Google IMAP/SMTP](https://developers.google.com/workspace/gmail/imap/imap-smtp)
- Sending limits: 2,000 messages/day on paid Workspace, 500/day on trial and personal accounts, up to 100 recipients per SMTP message. Limits are per user over a rolling 24 hours and shared across clients — [Google IMAP/SMTP](https://developers.google.com/workspace/gmail/imap/imap-smtp)
- IMAP bandwidth: 2,500 MB/day download, 500 MB/day upload. Hitting the sync limit temporarily pauses IMAP uploads. Google's best practice is to limit IMAP-visible labels to 500 — [Gmail bandwidth limits](https://knowledge.workspace.google.com/admin/gmail/gmail-bandwidth-limits)

**X-GM-EXT-1**
- `X-GM-RAW` gives full Gmail search syntax in SEARCH. `X-GM-MSGID` is a 64-bit message ID, `X-GM-THRID` a 64-bit thread ID. `X-GM-LABELS` can be fetched, stored and searched, and labels act as folders for CREATE/RENAME/DELETE — [Gmail IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)
- These work over OAuth. Clients should send the `ID` command with a contact address. `XLIST` has been deprecated since 2013 in favour of SPECIAL-USE, and Gmail adds an extra `\Important` attribute — [Gmail IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)

**Gmail REST API (quotas revised 1 May 2026)**
- Limits: 1,200,000 units/min per project, 6,000 units/min per user per project, 80,000,000 units/day per project as a billing threshold — [Gmail API quota](https://developers.google.com/workspace/gmail/api/reference/quota)
- Unit costs: `history.list` 2; `messages.list` and `messages.modify` 5; `messages.get` 20 (60 with full/raw/html format); `messages.attachments.get` 20; `threads.get` 40 (120 full); `messages.batchModify` 50; `messages.send` and `watch` 100 — [Gmail API quota](https://developers.google.com/workspace/gmail/api/reference/quota)
- `history.list`: records are typically kept at least a week but can be much shorter, with no fixed retention guarantee. An out-of-range `startHistoryId` returns HTTP 404 and forces a full sync — [Gmail sync guide](https://developers.google.com/workspace/gmail/api/guides/sync)
- Push goes through Cloud Pub/Sub (topic, subscription, publish rights for `gmail-api-push@system.gserviceaccount.com`). Pull or webhook delivery are both possible. `watch` must be renewed at least every 7 days (daily recommended). Each user is limited to one event per second and excess notifications are dropped. `labelIds` filters notifications — [Gmail push](https://developers.google.com/workspace/gmail/api/guides/push)

**Scope classification (Google's page carries no date)**

| Class | Scopes |
|---|---|
| Non-sensitive | `gmail.labels`, add-on action scopes |
| Sensitive | `gmail.send`, add-on metadata/readonly scopes |
| Restricted | `https://mail.google.com/`, `gmail.readonly`, `gmail.compose`, `gmail.insert`, `gmail.modify`, `gmail.metadata`, `gmail.settings.basic`, `gmail.settings.sharing` |

Source: [Gmail API scopes](https://developers.google.com/workspace/gmail/api/auth/scopes)
- Google's restricted list states that `mail.google.com` covers any usage of IMAP, SMTP and POP3 — [Restricted scopes](https://support.google.com/cloud/answer/13464325)
- The scopes page says to request `mail.google.com` only if permanent deletion is required — [Gmail API scopes](https://developers.google.com/workspace/gmail/api/auth/scopes)

**When verification applies**
- Exceptions: personal use (you or a few people you know, user cap applies); dev/test in "Testing" status; service-owned data; Internal user type within your own Workspace or Cloud Identity org. Domain-wide installation alone is not an exemption — [Restricted scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification)
- Personal-use wording: "fewer than 100 users", and those users can get past the unverified warning screens at sign-in. More than 100 requires verification — [When verification is not needed](https://support.google.com/cloud/answer/13464323)
- The cap is "100 new users in total", not concurrent users — [Unverified apps](https://support.google.com/cloud/answer/7454865)
- Approved restricted-scope Gmail use cases include email clients, worded as "Built-in and web email clients that allow users to compose, send, read, and process email via a user interface". One-time or manual export tools are prohibited — [Workspace API user data policy](https://developers.google.com/workspace/workspace-api-user-data-developer-policy)
- Restricted scopes also need a Limited Use compliant privacy policy. Verification can take several weeks, largely because of the assessment — [Restricted scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification)

**Testing mode and refresh tokens**
- "Testing" projects get refresh tokens expiring in 7 days, unless only basic name/email/profile scopes are requested. Other invalidation triggers: unused for six months; over 100 refresh tokens per account per client ID (oldest silently dropped); password change when the token has Gmail scopes; expired time-based access — [Google OAuth 2.0 overview](https://developers.google.com/identity/protocols/oauth2)
- A maintainer guide and a Google Groups thread both say the fix for 7-day expiry is moving to "In production", and that publishing unverified does not lift the 100-user cap — [TideMail wiki](https://github.com/allisonhere/tidemail/wiki/Google-OAuth-release); [Google Groups thread](https://groups.google.com/g/adwords-api/c/QTdeilIh_QY/m/eOHg0Q9CAgAJ)
- A forum post claiming test-mode tokens last indefinitely if used every six months contradicts Google's documentation and should be disregarded — [Latenode forum](https://community.latenode.com/t/how-to-get-permanent-refresh-token-for-personal-gmail-api-application/35300); contradicted by [Google OAuth 2.0 overview](https://developers.google.com/identity/protocols/oauth2)

**CASA: when it is required (sources conflict)**
- Developer docs: required for apps that access restricted-scope data and can access it from or through a third-party server — [Restricted scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification)
- Help Center: "applications requesting access to restricted scopes must undergo an annual security assessment", with no local vs server distinction — [Security assessment](https://support.google.com/cloud/answer/13465431)
- Policy: Google "may" require a periodic assessment and a Letter of Assessment. The policy has no Gmail-specific rule on local-only storage — [Workspace API user data policy](https://developers.google.com/workspace/workspace-api-user-data-developer-policy)
- Re-verification and assessment recur at least every 12 months from the Letter of Assessment date — [Restricted scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification)
- Apps are assigned AL1 or AL2 and do not choose. Once validated at AL2, an app stays there — [Security assessment](https://support.google.com/cloud/answer/13465431)

**CASA: cost (no official Google figure; all vendor or blog sources)**
- TAC Security list prices reported as $675 to $855 one-time AL1, $4,500 Enterprise AL1, $5,400 AL2. The article's dates are internally inconsistent and I did not verify the prices at TAC — [Deepstrike](https://deepstrike.io/blog/google-casa-security-assessment-2025)
- Other estimates: about $500 to $4,500, repeating annually, each lab setting its own price — [Aiemaily](https://aiemaily.com/blog/google-casa-verification-explained-for-buyers). TAC Tier 2 at $540 to $1,800/year, with the often-quoted $50,000 to $75,000 described as top-tier premium pricing rather than typical — [Bright Softwares](https://bright-softwares.com/blog/en/google-workspace/the-50000-gmail-add-on-myth-what-google-s-casa-certification-really-costs)
- Old tier names map roughly as AL0 = Tier 1, AL1 = Tier 2, AL2 = Tier 3 — [Deepstrike](https://deepstrike.io/blog/google-casa-security-assessment-2025)

**App passwords as a no-OAuth fallback**
- Still offered, and require 2-Step Verification. May be unavailable on work/school accounts, with Advanced Protection, or when 2SV uses only security keys. Revoked on password change. Google says they "aren't recommended" — [Google app passwords](https://support.google.com/accounts/answer/185833)

### Inferences
- **Scope choice for commercial review.** A REST client can ask for `gmail.modify` + `gmail.send`. An IMAP client must ask for `mail.google.com`, the broadest scope, which Google says to request only for permanent deletion. So IMAP may be a harder justification in review, though no source confirms a rejection on that basis.
- **Personal-use recipe.** Use External user type, publishing status "In production", no verification, and click through the warning. Google ties the 7-day expiry to Testing status, so this should give durable tokens within the lifetime 100-user cap. Test it on a real account before relying on it.
- **Bring-your-own client ID.** Each user creating their own Cloud project keeps every project inside personal use. It is a poor experience for non-developers and I found no Google statement blessing it for a distributed product.
- **Throughput under the May 2026 quota.** 6,000 units/min means about 100 `messages.get` calls/min at format=raw or full (60 units), or 300/min at 20 units. A 50,000-message mailbox takes about 8.3 hours of raw fetching. Over IMAP, 2,500 MB/day means a 10 GB mailbox takes at least 4 days. Initial sync needs a recent-first design on either path.
- **Quota figures in older posts** may predate the 1 May 2026 change. Use only the current page.
- **Polling `history.list`** every 60 seconds costs 2 units/min, so REST needs no push, and IMAP IDLE needs no Pub/Sub at all.
- **Token lifetime.** A client must plan for hourly IMAP disconnects under OAuth, including IDLE connections.

### Gaps
- No Google statement resolves whether a server-less desktop client needs CASA. Ask Google during verification rather than assuming. This is the largest cost unknown for the commercial path.
- "Built-in and web email clients" does not plainly say desktop clients. Third-party desktop clients are presumably covered, but the text is ambiguous.
- No official Gmail IMAP concurrent-connection limit was found. The bandwidth page does not state one.
- Google's published cost for verification itself, and any official CASA price, were not found.
- Workspace admin controls over third-party OAuth apps were not researched.
- Google's pages were read through a summarising fetch tool, so check the short quoted phrases against the live pages before reusing them verbatim.

## Microsoft: IMAP/SMTP OAuth, Basic Auth, SMTP AUTH timeline, Graph, Entra registration and consent

### Takeaway
Basic Auth is gone for Outlook.com (September 2024). In Exchange Online it survives only for SMTP AUTH, turning off by default at the end of December 2026. OAuth on IMAP/SMTP works for both account types, but Graph is the stronger sync path because it has per-folder delta and Exchange IMAP lacks CONDSTORE. Graph push still needs a public endpoint, so a desktop client would poll.

### Cited Findings

**Endpoints and scopes**
- OAuth2 for IMAP, POP and SMTP is available to both Microsoft 365 and Outlook.com users. Scopes: `https://outlook.office.com/IMAP.AccessAsUser.All`, `https://outlook.office.com/SMTP.Send`, plus `offline_access` for refresh tokens. SASL XOAUTH2 uses `base64("user=" + userName + "^Aauth=Bearer " + accessToken + "^A^A")`. For a shared mailbox, put the shared address in `user=` with a token obtained for the signed-in user — [Microsoft OAuth for IMAP/POP/SMTP](https://learn.microsoft.com/en-us/exchange/client-developer/legacy-protocols/how-to-authenticate-an-imap-pop-smtp-application-by-using-oauth) (page updated 17 Oct 2025)
- Microsoft 365: IMAP `outlook.office365.com:993` SSL/TLS, SMTP `smtp.office365.com:587` STARTTLS — [POP3 and IMAP4 in Exchange Online](https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/pop3-and-imap4/pop3-and-imap4) (page last updated Feb 2024)
- Outlook.com: IMAP `outlook.office365.com:993`, SMTP `smtp-mail.outlook.com:587` STARTTLS, "requires the use of Modern Auth / OAuth2". POP and IMAP are disabled by default, and the user must turn them on in Settings > Mail > Forwarding and IMAP — [Outlook.com settings](https://support.microsoft.com/en-us/office/pop-imap-and-smtp-settings-for-outlook-com-d088b986-291d-42b8-9564-9c414e2aa040)
- That same page still says an app password may be needed in some cases, which sits awkwardly with the OAuth requirement — [Outlook.com settings](https://support.microsoft.com/en-us/office/pop-imap-and-smtp-settings-for-outlook-com-d088b986-291d-42b8-9564-9c414e2aa040)

**Basic Auth status**
- Outlook.com: Basic Auth has been unavailable for any Outlook account since 16 September 2024 — [Microsoft Support](https://support.microsoft.com/en-us/office/modern-authentication-methods-now-needed-to-continue-syncing-outlook-email-in-non-microsoft-email-apps-c5d65390-9676-4763-b41f-d7986499a90d)
- Exchange Online SMTP AUTH, revised timeline published 27 Jan 2026, last updated 29 Jan 2026:
  - Through December 2026: behaviour unchanged.
  - End of December 2026: disabled by default for existing tenants, and admins can re-enable it.
  - New tenants after December 2026: unavailable by default, OAuth is the supported method.
  - Second half of 2027: Microsoft announces the final removal date.

  Source: [Exchange Team blog](https://techcommunity.microsoft.com/blog/exchange/updated-exchange-online-smtp-auth-basic-authentication-deprecation-timeline/4489835)
- The earlier plan (start 1 March 2026, 100% rejection 30 April 2026) is on hold — [MC786329](https://mc.merill.net/message/MC786329)
- Beware stale pages. The original blog post still shows the old dates alongside a January 2026 note — [original post](https://techcommunity.microsoft.com/blog/exchange/exchange-online-to-retire-basic-auth-for-client-submission-smtp-auth/4114750). A 2026 Mailbird guide also repeats March/April 2026 — [Mailbird](https://www.getmailbird.com/microsoft-modern-authentication-enforcement-email-guide/)
- The removal covers Basic auth only. Nothing in the post retires SMTP AUTH itself — [Exchange Team blog](https://techcommunity.microsoft.com/blog/exchange/updated-exchange-online-smtp-auth-basic-authentication-deprecation-timeline/4489835). One vendor says that as of October 2026 Microsoft's deprecation page still points to the January timeline — [Netsys](https://www.netsysgroup.com/blog/smtp-auth-basic-authentication-microsoft-365)
- The rejection error is `550 5.7.30 Basic authentication is not supported for Client Submission` — [original post](https://techcommunity.microsoft.com/blog/exchange/exchange-online-to-retire-basic-auth-for-client-submission-smtp-auth/4114750)
- EWS is not an option. Phased disablement began 1 October 2026 and EWS is permanently removed 1 April 2027, Exchange Online only — [MC1227454](https://mc.merill.net/message/MC1227454) (5 Feb 2026)

**Exchange Online limits**
- 30 messages/minute and 10,000 recipients/day. Over-rate SMTP client submissions are rejected and must be retried — [Exchange Online limits](https://learn.microsoft.com/en-us/office365/servicedescriptions/exchange-online-service-description/exchange-online-limits) (updated 19 Oct 2026)
- IMAP clients see "Request is throttled" with a suggested backoff. One developer hit it syncing about 2,000 folders through MailKit — [Tech Community thread](https://techcommunity.microsoft.com/t5/office-365/the-imap-server-replied-to-the-examine-command-with-a-no/td-p/2347133)
- Microsoft notes a several-second delay on each IMAP access because of a proxy hop, and describes IMAP as giving "basic email features" — [POP3 and IMAP4 in Exchange Online](https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/pop3-and-imap4/pop3-and-imap4)
- Admins can disable IMAP per user, and with security defaults on, POP3 and IMAP4 are "automatically disabled" — [POP3 and IMAP4 in Exchange Online](https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/pop3-and-imap4/pop3-and-imap4)

**Microsoft Graph Mail**
- Limits per app ID and mailbox: 10,000 requests per 10 minutes, 4 concurrent requests, 150 MB upload per 5 minutes. Global: 130,000 requests per 10 seconds per app across all tenants — [Graph throttling limits](https://learn.microsoft.com/en-us/graph/throttling-limits) (updated 17 Sep 2026)
- Delta is per folder: `/me/mailFolders/{id}/messages/delta`. It works with delegated work/school and personal accounts (`Mail.ReadBasic`, `Mail.Read`, `Mail.ReadWrite`). `$select`, `$top` and `$expand` are supported. `$filter` is limited to `receivedDateTime ge/gt`, `$orderby` to `receivedDateTime desc`, and there is no `$search`. Deleted or moved items appear as `@removed` — [message: delta](https://learn.microsoft.com/en-us/graph/api/message-delta) (updated 19 Jun 2026)
- Change notifications for messages: max subscription lifetime 10,080 minutes (under 7 days), or 1,440 minutes for rich notifications. 1,000 active subscriptions per mailbox. Average latency under 1 minute, maximum 3 minutes. Channels are webhooks, Event Hubs and Event Grid — [Change notifications](https://learn.microsoft.com/en-us/graph/change-notifications-overview) (updated 7 Apr 2026)
- Webhooks need a "publicly accessible, HTTPS-secured endpoint". Otherwise Graph sends nothing. Validation must answer within 10 seconds, and delivery must be acknowledged within 3 seconds — [Webhook delivery](https://learn.microsoft.com/en-us/graph/change-notifications-delivery-webhooks) (updated 22 Apr 2026)

**Entra app registration**
- Since June 2024 all new apps must be registered in a directory. A personal account can create one by joining the Microsoft 365 Developer Program or signing up for Azure. Existing apps are unaffected — [Entra breaking changes](https://learn.microsoft.com/en-us/entra/identity-platform/reference-breaking-changes) (updated 26 Aug 2026, no newer relevant entries)
- One Q&A case shows a personal-account tenant blocked for inactivity (AADSTS5000225) and later reactivated. It is a single case, not policy — [Microsoft Q&A](https://learn.microsoft.com/en-us/answers/questions/5811045/account-locked-cannot-access-entra-id-portal-or-cr)
- Desktop apps use the "Mobile and desktop applications" platform. An audience of work/school plus personal accounts allows up to 100 redirect URIs, and query parameters in redirect URIs are not allowed for it — [Redirect URI rules](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)

**Consent and publisher verification**
- By default users can consent to an app accessing their own mailbox. Admins can restrict consent to verified publishers with low-impact permissions (`microsoft-user-default-low`) or disable it — [User consent settings](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/configure-user-consent) (updated 4 Aug 2026)
- Where risk-based step-up consent is enabled, users cannot consent to most multitenant apps registered after 8 Nov 2020 that are unverified and request more than basic sign-in — [Publisher verification](https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview) (updated 15 Jun 2026)
- Publisher verification is free and needs no license, but requires:
  - a verified Cloud Partner Program account (partner global account);
  - an app registered with a work or school account, as apps registered with a Microsoft account cannot be verified;
  - a publisher domain that is not `*.onmicrosoft.com`, matching the partner verification email or a DNS-verified domain.

  Source: [Publisher verification](https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview)
- Thunderbird shows the real-world tenant problem: Mozilla publishes an admin-consent URL with its client ID for organisations — [Mozilla support](https://support.mozilla.org/en-US/kb/microsoft-oauth-authentication-and-thunderbird-202/revision/275611) (non-current revision)

### Inferences
- **No Microsoft equivalent to CASA turned up.** A one-developer app can register for free, and personal accounts plus permissive tenants work unverified. The friction appears only in tenants that restrict consent.
- **Publisher verification needs a business-like footing** (partner account, owned domain). Plan it for the commercial stage. Registering early in a directory using a work/school identity avoids re-registering later.
- **End of December 2026 does not affect an OAuth client.** It is about 8 weeks away, but it matters only for anyone planning password fallback on Microsoft 365, which should simply not be built.
- **Graph avoids two user-facing failures:** the Outlook.com "enable IMAP" toggle and tenant-disabled IMAP. This is my reading, since these settings govern the IMAP protocol, and it needs a test.
- **Polling cost is trivial.** Delta on 10 folders every 60 seconds is 100 requests per 10 minutes, or 1% of the budget. The 4-concurrent limit is the real constraint during initial sync.
- **Tokens are per resource.** IMAP/SMTP use `outlook.office.com` scopes and Graph uses Graph scopes, so a mixed design (for example, IMAP read with Graph send) needs separate access tokens. Avoid it.

### Gaps
- No official Exchange Online or Outlook.com concurrent IMAP connection limit was found. The often-repeated "20 connections" figure is unverified, and the only Microsoft number found was an old on-premises default of 10 — [Exchange 2010 docs](https://learn.microsoft.com/en-us/previous-versions/exchange-server/exchange-140/bb123712(v=exchg.140))
- The security-defaults note sits in a Feb 2024 page next to Basic Auth text, so it is unclear whether OAuth IMAP is also blocked. Test on a tenant.
- Outlook.com app-password status is contradictory. Treat OAuth as the only supported route.
- A July 2026 Q&A thread reports both success and failure for Thunderbird OAuth on Outlook.com, so consumer reliability is anecdotal — [Microsoft Q&A](https://learn.microsoft.com/en-us/answers/questions/5954774/outlook-com-authentication-problem)
- Whether an individual without a company can obtain a partner global account was not established.
- Windows broker (WAM) sign-in through MSAL was not researched.
- Consumer Outlook.com sending limits were not confirmed. The limits cited above are for Exchange Online.

## Apple iCloud Mail: authentication, API availability, servers, limits, quirks

### Takeaway
App-specific passwords over IMAP/SMTP remain the only documented third-party route, with no public OAuth or REST API. iCloud's servers do advertise `AUTH=XOAUTH2`, but I found no documentation that third parties can use it. iCloud's IMAP has QRESYNC, reportedly with an ENABLE bug that silently breaks some libraries.

### Cited Findings
- IMAP: `imap.mail.me.com:993` SSL. SMTP: `smtp.mail.me.com:587` with SSL/TLS or STARTTLS, authentication required. The IMAP username is the name without the domain (full address if that fails). The SMTP username is the full address. The same app-specific password serves both. No POP — [Apple mail settings](https://support.apple.com/en-us/102525)
- App-specific passwords require two-factor authentication, are created at account.apple.com, are capped at 25 active, and are all revoked when the Apple Account password changes or resets — [Apple app-specific passwords](https://support.apple.com/en-us/102654)
- They cannot be generated programmatically, and Apple publishes no REST or HTTP API for iCloud Mail — [Nylas iCloud guide](https://developer.nylas.com/docs/cookbook/email/icloud-mail-api.md) (third party; I found nothing from Apple contradicting it)
- Limits: 1,000 messages/day, 1,000 recipients/day, 500 recipients/message, 20 MB per message (up to 5 GB with Mail Drop). 5 GB free storage is shared with other iCloud data, and when full, mail cannot be sent or received — [Apple iCloud Mail limits](https://support.apple.com/en-us/102198)
- Local probe, 4 Nov 2026, unauthenticated, port 993, repeated with two TLS stacks: `CAPABILITY XAPPLEPUSHSERVICE IMAP4 IMAP4rev1 SASL-IR AUTH=ATOKEN AUTH=PLAIN AUTH=ATOKEN2 AUTH=XOAUTH2`
- Local probe, 4 Nov 2026, `smtp.mail.me.com:587` after STARTTLS: `SIZE 28319744`, `AUTH LOGIN PLAIN ATOKEN ATOKEN2 WSTOKEN WETOKEN XOAUTH2`, `SMTPUTF8`, `CHUNKING`, `DSN`, `8BITMIME`, `PIPELINING`
- Post-login capabilities as reported on 2 Nov 2026 include CONDSTORE, ENABLE, QRESYNC, QUOTA, UIDPLUS, ESEARCH and IDLE (the published line is abbreviated) — [imapflow #457](https://github.com/postalsys/imapflow/issues/457)
- **ENABLE quirk.** `ENABLE CONDSTORE QRESYNC` returns tagged OK with no untagged `* ENABLED`. imapflow then silently dropped `changedSince` and returned all 35,992 messages rather than 1,081. After forcing the enabled state, `CHANGEDSINCE`, `VANISHED (EARLIER)` and `SELECT ... (QRESYNC ...)` all worked. This is one report with no maintainer reply yet — [imapflow #457](https://github.com/postalsys/imapflow/issues/457)

### Inferences
- `AUTH=XOAUTH2` plus the ATOKEN mechanisms probably serve Apple's own clients and services. Its presence is not evidence of a third-party OAuth program. I did not attempt it, and there is no registration route to obtain such a token.
- SMTP `SIZE` (about 27 MiB) exceeding the 20 MB documented limit is consistent with allowing for base64 overhead. Enforce Apple's 20 MB in the UI.
- Onboarding needs a guided flow linking to account.apple.com, and the client must treat auth failure after a password change as "generate a new app password". No token refresh code is needed.
- Treat advertised capability as sufficient for iCloud rather than requiring `* ENABLED`, and write an integration test that asserts a CHANGEDSINCE fetch really returns a subset.

### Gaps
- No Apple documentation of IMAP capabilities, IDLE timeouts, concurrent connection limits or bandwidth limits was found. Forum-level figures were not reliable enough to cite.
- Nothing was found on Thunderbird, mbsync or Delta Chat behaviour specifically against iCloud.
- Post-login capabilities were not verified first-hand, because that needs a real account.

## IMAP extension support and known quirks by provider

### Takeaway
Only IDLE and UIDPLUS are common to all three. Gmail has CONDSTORE but no QRESYNC, iCloud has QRESYNC but reportedly no MOVE or SPECIAL-USE, and Exchange has MOVE but no CONDSTORE. The engine must therefore support three different sync strategies.

### Cited Findings

Matrix, post-login where known:

| Extension | Gmail | Exchange Online / Outlook | iCloud |
|---|---|---|---|
| IDLE | Yes | Yes | Yes |
| UIDPLUS | Yes | Yes | Yes |
| MOVE | Yes | Yes | **No** (undated) |
| CONDSTORE | Yes | **No** | Yes |
| QRESYNC | **No** | **No** | Yes |
| ENABLE | Yes | **No** | Yes (buggy) |
| COMPRESS=DEFLATE | Yes | **No** | **No** |
| SPECIAL-USE | Yes | **No** | **No** (undated) |
| ESEARCH | Yes | **No** | Yes |
| LIST-STATUS | Yes | **No** | Yes |
| UTF8=ACCEPT | Yes | **No** | **No** |
| NOTIFY | Not listed | Not listed | Not listed |
| OBJECTID | Not listed | Not listed | Not listed |

- Primary matrix source. Only the Exchange column is dated (captured 2026-03-05 from `outlook.office365.com:993`). The Gmail and iCloud columns have no stated capture date — [SwiftMail README](https://github.com/Cocoanetics/SwiftMail)
- Gmail cross-check, August 2024: `IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 UIDPLUS COMPRESS=DEFLATE ENABLE MOVE CONDSTORE ESEARCH UTF8=ACCEPT LIST-EXTENDED LIST-STATUS LITERAL- SPECIAL-USE APPENDLIMIT=35651584` — [emersion gist comment](https://gist.github.com/emersion/2c769bc1ed60a7b7945910d35b606801)
- Gmail's pre-login list is shorter and differs, for example carrying `XYZZY` and `SASL-IR` and lacking UIDPLUS and MOVE. So re-read capabilities after authenticating — [emersion gist](https://gist.github.com/emersion/2c769bc1ed60a7b7945910d35b606801); [Limilabs](https://www.limilabs.com/blog/gmail-special-use-capability-is-broken)
- Exchange shows the same pattern: MOVE appears only after authentication in 2020 to 2022 logs. This comes from a search summary across several blogs, and I did not open the individual log, so the link is the likeliest of those sources rather than a confirmed one — [eightwone](https://eightwone.com/2020/07/01/configuring-exchange-account-with-imap-oauth2/)
- Exchange parser trap: the server can advertise `AUTH=XOAUTH2` alongside `LOGINDISABLED`. One client library aborted on LOGINDISABLED, though it only blocks plaintext LOGIN — [node-red-nodes #1152](https://github.com/node-red/node-red-nodes/issues/1152)
- A server advertising QRESYNC implies CONDSTORE. Long UID sets can hit line-length limits, so clients should cap command length — [RFC 7162 draft text](https://datatracker.ietf.org/doc/html/draft-ietf-qresync-rfc5162bis)

### Inferences
- **Sync tiers.** QRESYNC for iCloud; CONDSTORE plus `X-GM-MSGID`-based reconciliation for Gmail, since without QRESYNC, expunges need a UID comparison; and full UID/flag scans for Exchange. The Exchange scan grows with folder size and is the main technical argument for Graph.
- **Folder role detection** needs a name-based fallback for Exchange and iCloud if SPECIAL-USE is truly absent there.
- **MOVE fallback** (COPY, STORE `\Deleted`, UID EXPUNGE) is needed for iCloud.
- **Gmail semantics.** Labels mean one message appears in several folders. Deduplicate on `X-GM-MSGID`, and map move/delete to label operations.
- **NOTIFY and OBJECTID** should not be designed around. Multi-folder change detection means one IDLE connection per watched folder or periodic polling.

### Gaps
- **Live probes for Gmail and Microsoft could not be run.** Port 993 to `imap.gmail.com`, `outlook.office365.com` and `imap-mail.outlook.com`, and port 587 to their SMTP hosts, timed out from this machine with two tools while iCloud connected. That is a limit of this network path and says nothing about the servers.
- Post-login lists need one authenticated `CAPABILITY` per provider, particularly to confirm iCloud MOVE/SPECIAL-USE and Exchange SPECIAL-USE.
- NOTIFY/OBJECTID absence is inferred from published lists, and the iCloud line available was abbreviated. The Gmail list is from 2024.
- The SwiftMail table shows Gmail without SASL-IR, while the gist shows it pre-login. This looks like a pre/post-login difference and was not resolved.
- A Limilabs post titled "Gmail's SPECIAL-USE capability is broken" appeared in results. I read only a summary, and it may be dated.

## Is JMAP relevant to these three providers?

### Takeaway
No. I found no evidence that Gmail, Outlook or iCloud offers JMAP, so it adds nothing to this decision.

### Cited Findings
- Fastmail runs JMAP in production, and server support exists in Cyrus, Apache James and Stalwart. A forum commenter notes that Google and Microsoft use IMAP and their own APIs — [Comparison of webmail providers](https://en.wikipedia.org/wiki/Comparison_of_webmail_providers); [eM Client forum](https://forum.emclient.com/t/jmap-support-wanted/78419/17)
- iCloud's JMAP status is listed as unknown in that comparison — [Comparison of webmail providers](https://en.wikipedia.org/wiki/Comparison_of_webmail_providers)

### Inferences
- A provider-neutral sync interface, justified anyway by Graph, would leave room for a later Fastmail/JMAP adapter at no extra cost today.

### Gaps
- This is absence of evidence from weak sources (forum posts, Wikipedia). No provider states it directly, and JMAP discovery endpoints were not probed.

## Native-app OAuth best practice and client secrets

### Takeaway
Use the system browser, a loopback redirect on `127.0.0.1`, an ephemeral port and PKCE S256 for both. Microsoft needs no secret. For Google, assume the Desktop client secret must ship in the binary, and treat it as non-secret, because reports through October 2026 show the token endpoint demanding it despite docs calling it optional.

### Cited Findings

**RFC 8252**
- Native apps MUST use an external user-agent and MUST NOT use embedded web views. Public native clients MUST implement PKCE. Authorization servers MUST accept any port on loopback redirects. Use `127.0.0.1` or `[::1]` rather than `localhost`, open the port only for the request, and bind only to loopback — [RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.txt)
- Secrets shipped in a distributed app must not be treated as confidential, and requiring one for public native apps is NOT RECOMMENDED — [RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.txt)

**Google**
- Loopback IP is the recommended method on Windows, using `http://127.0.0.1:port` or `http://[::1]:port`. `localhost` can work but risks firewall problems. Custom URI schemes and copy/paste (out-of-band) are no longer supported. Always use PKCE, with S256 recommended. Refresh tokens are always returned for installed apps, and incremental authorization is not supported for them — [Google native app OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)
- Docs list `client_secret` as optional and state "Desktop apps are public clients and cannot keep secrets confidential" — [Google native app OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)
- Contradicted in practice: multiple developers report HTTP 400 `invalid_request` "client_secret is missing" for Desktop clients with PKCE, including new clients, in posts dated August to October 2026. No Google staff reply or resolution appears. One poster's diagnostics were inconclusive — [Google developer forum](https://discuss.google.dev/t/desktop-oauth-pkce-exchange-returns-invalid-request-after-successful-loopback-callback-with-no-client-secret/390526)
- A community reply quotes Google as saying that "in this context, the client secret is obviously not treated as a secret". I did not confirm that wording at source — [Google developer forum](https://discuss.google.dev/t/desktop-oauth-pkce-exchange-returns-invalid-request-after-successful-loopback-callback-with-no-client-secret/390526)
- An open-source CLI hit the same error and ships the Desktop secret — [gtasks auth package](https://pkg.go.dev/github.com/yanicksenn/workspace/go/cmd/gtasks/internal/auth)

**Microsoft**
- Public clients "can't have client secrets", and desktop apps are public clients — [MSAL client types](https://learn.microsoft.com/en-us/entra/identity-platform/msal-client-applications)
- Loopback port is ignored when matching, so register once without a port. `[::1]` isn't supported. Microsoft prefers `127.0.0.1` over `localhost`, but `http://127.0.0.1` can't be added in the portal text box and needs the `replyUrlsWithType` manifest attribute. Don't register multiple localhost URIs differing only by port — [Redirect URI rules](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)
- The same page says the http scheme for localhost "should be used only during active local application development", yet its RFC 8252 section accepts it for native apps. The native-app reading applies here — [Redirect URI rules](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)

### Inferences
- Google's secret in the binary carries little security value. A leak lets someone impersonate the client ID, not access any mailbox. Keep the IDs and the secret in config so they can rotate without a code change.
- Do not add a backend to hide the Google secret. Routing token exchange or data through a server is the condition that most clearly triggers CASA, so it would add cost in exchange for negligible security.
- Store refresh tokens with Windows DPAPI or Credential Manager rather than in plain files. This is general practice and was not specifically researched.
- A community suggestion to use an iOS-type client for desktop is an unsupported workaround and should not be used.

### Gaps
- Whether Google's secret requirement is deliberate or varies by client is unresolved. Test a new Desktop client with and without the secret.

## Recommendation: single IMAP/SMTP engine or native adapters, and what forces a backend

### Takeaway
Build one IMAP/SMTP engine and ship all three providers on it, with per-provider auth and quirk profiles and a sync interface that allows other backends. Add a Graph adapter as the first and likely only native adapter. Add a Gmail REST adapter only if commercial review makes `mail.google.com` a problem. No backend is needed.

### Cited Findings
Evidence is in the sections above. The deciding facts are:
- Gmail IMAP and REST both require restricted scopes, so REST buys no verification saving — [Gmail API scopes](https://developers.google.com/workspace/gmail/api/auth/scopes)
- Exchange IMAP lacks CONDSTORE and QRESYNC (captured 5 Mar 2026) — [SwiftMail README](https://github.com/Cocoanetics/SwiftMail); Graph offers per-folder delta for work and personal accounts — [message: delta](https://learn.microsoft.com/en-us/graph/api/message-delta)
- iCloud has no alternative to IMAP/SMTP — [Apple mail settings](https://support.apple.com/en-us/102525)
- Push on Gmail and Graph needs server-side receivers or Pub/Sub — [Gmail push](https://developers.google.com/workspace/gmail/api/guides/push); [Webhook delivery](https://learn.microsoft.com/en-us/graph/change-notifications-delivery-webhooks)

### Inferences

| Provider | Stage 1 (personal) | Native adapter worth it? | Why |
|---|---|---|---|
| iCloud | IMAP/SMTP, app-specific password | Not possible | No API exists |
| Gmail | IMAP + XOAUTH2 + X-GM-EXT-1, unverified "In production" app | Not at first; revisit at commercial stage | Same review tier, and X-GM-EXT-1 covers labels, threads and search |
| Microsoft | IMAP/SMTP + XOAUTH2 | **Yes, first candidate** | Delta sync replaces full flag scans, and avoids the IMAP toggles |

- **Gmail REST upside**, if ever built: `history.list` gives account-wide incremental sync at 2 units and narrower scopes. Downsides are a second data model and the tighter May 2026 per-user quota on bulk fetch.
- **Sequencing option.** If effort allows only one adapter early, Graph could replace Microsoft IMAP entirely rather than sit beside it, avoiding two Microsoft paths.
- **Real-time without a backend.** IMAP IDLE for Gmail and iCloud, IMAP IDLE or roughly 60-second delta polling for Microsoft. Everything is client-initiated.
- **What would force a backend:**
  - Gmail push by webhook. Pub/Sub pull avoids a public endpoint, but a desktop app would need Google Cloud credentials to pull, so it is not a realistic shipped design.
  - Graph webhooks, Event Hubs or Event Grid delivery.
  - Mobile or closed-app notifications.
  - Any server-side processing of mail (AI summaries, send-later while the app is closed, cross-device state).

  Each of these except pure wake-up signalling likely moves the product inside the CASA trigger, since user data would pass through a server.
- **Commercial triggers to plan for:**
  - Exceeding 100 lifetime Google users starts verification, and possibly annual CASA at a few hundred to about $5,400 per year on unverified vendor figures.
  - Microsoft enterprise sales need publisher verification.
  - Neither changes the engine choice.

### Gaps
- Effort estimates for each adapter were not researched.
- The recommendation rests on the unresolved CASA question only for the commercial Gmail cost, not for the architecture.
- Practical issue-tracker experience from aerc, mbsync, Mailspring, Delta Chat and Geary was largely not gathered within the research budget, so real-world quirks beyond those cited are under-represented.
