//! Finding the one-time code in a login mail, so that a notification can offer to copy it.
//!
//! A wrong code is worse than none, as it only shows when the login fails, so anything unclear
//! finds nothing and the mail is announced as any other. The rules follow Android's detector
//! (`NotificationOtpDetectionHelper` in AOSP) and were tightened against real mail: "code" alone
//! says little in an email, where promo codes, order codes and code reviews are everywhere.

use std::time::Duration;

use mail_parser::MessageParser;
use unicode_normalization::UnicodeNormalization;

/// How long a code is worth offering. Older mail is one that waited for the app to come back.
pub const LIFETIME: Duration = Duration::from_secs(15 * 60);

/// Codes come near the top. Past this is footers and whatever was quoted.
const MOST_TEXT: usize = 4000;
/// How far from a code, in characters, the words about it can be.
const CONTEXT: usize = 60;
/// How close a word has to be to say what a number really is.
const LABEL: usize = 30;

/// What a security code is called, in English, Vietnamese, Spanish, Portuguese, French, German,
/// Chinese, Japanese and Korean. In a subject these point to a code, and beside a number they say
/// it is one. Other languages go without the button, which is the safe way to miss.
const CODE_NAMES: &[&str] = &[
    "verification code", "verify code", "security code", "login code", "log-in code", "log in code", "sign-in code",
    "sign in code", "signin code", "authentication code", "access code", "confirmation code", "recovery code", "pin code",
    "verification pin", "passcode", "2fa", "one-time code", "one time code", "one-time password", "one time password",
    "one-time passcode", "one time passcode", "otp",
    "mã xác thực", "mã xác nhận", "mã xác minh", "mã otp", "mã đăng nhập", "mã bảo mật",
    "código de verificación", "código de seguridad", "código de acceso", "código de inicio de sesión",
    "código de confirmación", "código de un solo uso",
    "código de verificação", "código de segurança", "código de acesso", "código de confirmação", "código de login",
    "code de vérification", "code de sécurité", "code de confirmation", "code d'accès", "code de connexion",
    "code à usage unique",
    "bestätigungscode", "verifizierungscode", "sicherheitscode", "anmeldecode", "einmalcode", "zugangscode", "einmalpasswort",
    "验证码", "驗證碼", "校验码", "动态码", "动态密码", "安全码", "登录码", "登入碼", "认证码", "認證碼", "确认码", "確認碼",
    "認証コード", "確認コード", "検証コード", "ログインコード", "セキュリティコード", "ワンタイムコード", "ワンタイムパスワード",
    "인증번호", "인증 번호", "인증코드", "인증 코드", "확인코드", "확인 코드", "보안코드", "보안 코드", "로그인 코드",
];

/// A subject that points to a code without naming one.
const LOGIN_SUBJECTS: &[&str] = &[
    "one-time", "one time", "two-factor", "verify your", "sign in to", "log in to", "xác minh", "xác thực", "verifica tu",
    "confirma tu", "verifique seu", "confirme seu", "vérifiez votre", "confirmez votre", "bestätige deine", "bestätigen sie",
];

/// Travel mail calls its booking number a confirmation code.
const NOT_LOGIN_SUBJECTS: &[&str] = &[
    "booking", "flight", "reservation", "itinerary", "hotel", "trip", "đặt vé", "đặt phòng", "reserva", "vuelo", "voo",
    "réservation", "vol", "buchung", "flug", "预订", "預訂", "机票", "機票", "予約", "航空券", "예약", "항공권",
];

/// Words beside a number that say it is the code, as a name alone would not.
const CODE_WORDS: &[&str] = &[
    "code is", "code:", "code :", "code -", "code est", "code ist", "code lautet", "to sign in", "to log in", "mã của bạn",
    "código:", "código :", "código es", "código é",
];

/// Enough in a body the subject has called a login mail, but anywhere else these are about source
/// code as often as not.
const LOOSE_CODE_WORDS: &[&str] = &[
    "your code", "this code", "the code", "mã này", "tu código", "su código", "este código", "el código", "seu código",
    "o código", "votre code", "ce code", "le code", "ihr code", "dein code", "diesen code", "der code",
];

/// Say what a number is so plainly that nothing later in the sentence can change it.
const PLAINLY_A_CODE: &[&str] = &["otp", "one-time password", "one time password"];

/// Words that say the number beside them is something else.
const OTHER_NUMBERS: &[&str] = &[
    "promo", "coupon", "discount", "voucher", "zip", "postal", "tracking", "order", "invoice", "booking", "reference",
    "ref", "meeting", "customer", "gift", "tax", "approval", "transaction code", "transaction id", "transaction number",
    "transaction no", "ticket", "case", "id", "mã giảm", "mã đơn", "mã khuyến", "mã vận", "mã giao dịch", "mã giới thiệu",
    "pedido", "factura", "fatura", "reserva", "cupón", "cupom", "descuento", "desconto", "seguimiento", "rastreio",
    "commande", "facture", "réservation", "réduction", "suivi", "bestellung", "rechnung", "buchung", "gutschein", "rabatt",
    "订单", "訂單", "优惠", "優惠", "快递", "物流", "发票", "發票", "取件码", "注文", "クーポン", "請求", "追跡", "주문", "쿠폰", "송장",
    "배송",
];

/// Amounts, which read as codes when the sign is written after them.
const UNITS: &[&str] = &["points", "usd", "vnd", "vnđ", "đ", "eur"];

/// These scripts run words together, so each of their characters stands on its own, and a code can
/// touch them with no space: 验证码是482913.
fn is_unspaced(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{ac00}'..='\u{d7a3}' | '\u{f900}'..='\u{faff}')
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() && !is_unspaced(c)
}

/// Where a sentence ends, in any of the scripts above.
fn ends_sentence(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\n' | '。' | '！' | '？')
}

/// Whether a phrase is in the text as whole words.
fn says(text: &str, phrase: &str) -> bool {
    let word = |c: Option<char>| c.is_some_and(is_word);
    text.match_indices(phrase).any(|(at, _)| {
        !word(text[..at].chars().next_back()) && !word(text[at + phrase.len()..].chars().next())
    })
}

fn says_any(text: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|phrase| says(text, phrase))
}

fn folded(text: &str) -> String {
    text.nfc().flat_map(char::to_lowercase).collect()
}

fn is_reply(subject: &str) -> bool {
    ["re:", "fw:", "fwd:"].iter().any(|prefix| subject.trim_start().starts_with(prefix))
}

/// Whether this could be a login mail with its code in the body, and so one to read.
pub fn may_have_code(subject: &str) -> bool {
    let subject = folded(subject);
    let points_to_one = says_any(&subject, CODE_NAMES) || says_any(&subject, LOGIN_SUBJECTS);
    !is_reply(&subject) && points_to_one && !says_any(&subject, NOT_LOGIN_SUBJECTS)
}

/// A word as the code it could be, in the form to paste.
fn as_code(word: &str) -> Option<String> {
    let digits = word.bytes().filter(u8::is_ascii_digit).count();
    let is_year = |w: &str| w.len() == 4 && (w.starts_with("19") || w.starts_with("20"));
    if digits == word.len() {
        return ((4..=8).contains(&digits) && !is_year(word)).then(|| word.to_string());
    }
    // Google writes its codes as G-123456, and only the digits go in the box.
    if let Some((prefix, rest)) = word.split_once('-')
        && prefix.len() == 1
        && prefix.bytes().all(|b| b.is_ascii_uppercase())
        && rest.bytes().all(|b| b.is_ascii_digit())
    {
        return (4..=8).contains(&rest.len()).then(|| rest.to_string());
    }
    if word.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        // Any other shape of digits and dashes is a date or a phone number.
        let halves = word.split_once('-').filter(|(a, b)| a.len() == 3 && b.len() == 3);
        return halves.map(|(a, b)| format!("{a}{b}"));
    }
    let shape = word.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-');
    let length = word.bytes().filter(|&b| b != b'-').count();
    (shape && digits >= 2 && (5..=10).contains(&length)).then(|| word.to_string())
}

/// Text as characters, with a folded copy that lines up with it.
struct Page {
    chars: Vec<char>,
    folded: Vec<char>,
}

impl Page {
    fn new(text: &str) -> Self {
        let chars: Vec<char> = text.nfc().take(MOST_TEXT).collect();
        let folded = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
        Self { chars, folded }
    }

    fn at(&self, index: usize) -> Option<char> {
        self.chars.get(index).copied()
    }

    fn digit_at(&self, index: Option<usize>) -> bool {
        index.and_then(|i| self.at(i)).is_some_and(|c| c.is_ascii_digit())
    }

    fn is_gap(&self, index: usize) -> bool {
        self.at(index).is_some_and(|c| c == ' ' || c == '\u{a0}')
    }

    /// The words of the text that could be codes, by where they start and end.
    fn words(&self) -> Vec<(usize, usize)> {
        let mut words = Vec::new();
        let mut at = 0;
        while at < self.chars.len() {
            if !is_word(self.chars[at]) {
                at += 1;
                continue;
            }
            let start = at;
            while self.at(at).is_some_and(|c| is_word(c) || c == '-') {
                at += 1;
            }
            let mut end = at;
            while self.chars[end - 1] == '-' {
                end -= 1;
            }
            words.push((start, end));
        }
        words
    }

    fn word(&self, (start, end): (usize, usize)) -> String {
        self.chars[start..end].iter().collect()
    }

    /// Whether what touches a word on either side says it is part of something longer: a price,
    /// a version, a time, a handle, or a name such as 1234's.
    fn stands_alone(&self, start: usize, end: usize) -> bool {
        let before = start.checked_sub(1).and_then(|i| self.at(i));
        let two_before = start.checked_sub(2);
        let glued = matches!(before, Some('#' | '$' | '€' | '£' | '.' | ',' | '/' | '_' | '@'))
            || (before == Some(':') && self.digit_at(two_before));
        let after = self.at(end);
        let next = self.at(end + 1);
        let continues = matches!(after, Some('%' | '_' | '@'))
            || (matches!(after, Some('.' | ',' | ':' | '/')) && self.digit_at(Some(end + 1)))
            || (matches!(after, Some('\'' | '’')) && next.is_some_and(char::is_alphabetic));
        let rest: String = self.folded[end..(end + 8).min(self.folded.len())].iter().collect();
        !glued && !continues && !UNITS.iter().any(|unit| says(rest.trim_start(), unit) && rest.trim_start().starts_with(unit))
    }

    /// The words before a code, as far back as the sentence before its own.
    fn before(&self, start: usize) -> String {
        let from = start.saturating_sub(CONTEXT);
        let (mut sentences, mut cut, mut in_sentence) = (0, from, false);
        for index in (from..start).rev() {
            let c = self.folded[index];
            if ends_sentence(c) {
                if in_sentence {
                    sentences += 1;
                    in_sentence = false;
                }
                if sentences == 2 {
                    cut = index + 1;
                    break;
                }
            } else if !c.is_whitespace() {
                in_sentence = true;
            }
        }
        self.folded[cut..start].iter().collect()
    }

    /// The words after a code, to the end of its sentence.
    fn after(&self, end: usize) -> String {
        let to = (end + CONTEXT).min(self.folded.len());
        self.folded[end..to].iter().take_while(|&&c| !ends_sentence(c)).collect()
    }

    fn is_about_a_login(&self, start: usize, end: usize, known_login: bool) -> bool {
        let (before, after) = (self.before(start), self.after(end));
        let named = before.trim_end().ends_with("your code") || (after.trim_start().starts_with("is your") && says(&after, "code"));
        // A code often has a line to itself, with "This code expires" on the next.
        let next_line: String = self.folded[end..(end + CONTEXT).min(self.folded.len())].iter().collect();
        let loosely = known_login && (says_any(&before, LOOSE_CODE_WORDS) || says_any(&next_line, LOOSE_CODE_WORDS));
        let label: String = before.chars().rev().take(LABEL).collect::<Vec<_>>().into_iter().rev().collect();
        let says_it = |words| says_any(&before, words) || says_any(&after, words);
        let plainly = says_it(PLAINLY_A_CODE);
        !says_any(&label, OTHER_NUMBERS)
            && (plainly || !says_any(&after, OTHER_NUMBERS))
            && (named || loosely || says_it(CODE_NAMES) || says_it(CODE_WORDS))
    }

    /// The codes this text is about, each as it should be pasted.
    fn codes(&self, known_login: bool) -> Vec<String> {
        let words = self.words();
        let mut codes = Vec::new();
        let mut index = 0;
        while index < words.len() {
            let (start, end) = words[index];
            index += 1;
            let digits = |range: (usize, usize)| self.word(range).bytes().all(|b| b.is_ascii_digit());
            let beside = |a: (usize, usize), b: (usize, usize)| a.1 + 1 == b.0 && self.is_gap(a.1) && digits(a) && digits(b);
            let previous = index.checked_sub(2).map(|i| words[i]);
            let next = words.get(index).copied();
            let mut found = None;
            let mut last = end;
            if previous.is_some_and(|p| beside(p, (start, end))) {
                // The rest of a number already passed over: a phone number or a card.
            } else if let Some(pair) = next.filter(|&n| beside((start, end), n)) {
                let then = words.get(index + 1).copied();
                let threes = end - start == 3 && pair.1 - pair.0 == 3;
                if threes && !then.is_some_and(|t| beside(pair, t)) {
                    found = Some(format!("{}{}", self.word((start, end)), self.word(pair)));
                    last = pair.1;
                    index += 1;
                }
            } else {
                found = as_code(&self.word((start, end)));
            }
            if let Some(code) = found
                && self.stands_alone(start, last)
                && self.is_about_a_login(start, last, known_login)
            {
                codes.push(code);
            }
        }
        codes
    }
}

/// The one code in a text. Two different ones means there is no telling which is meant.
fn only_code(text: &str, known_login: bool) -> Option<String> {
    let mut codes = Page::new(text).codes(known_login);
    codes.sort();
    codes.dedup();
    (codes.len() == 1).then(|| codes.remove(0))
}

/// The code in a subject line, which needs no body to find.
pub fn in_subject(subject: &str) -> Option<String> {
    let folded = folded(subject);
    if is_reply(&folded) || says_any(&folded, NOT_LOGIN_SUBJECTS) {
        return None;
    }
    only_code(subject, false)
}

/// The code in a message, from its subject or else from the top of its text.
pub fn in_message(subject: &str, raw: &[u8]) -> Option<String> {
    in_subject(subject).or_else(|| {
        if !may_have_code(subject) {
            return None;
        }
        let parsed = MessageParser::default().parse(raw)?;
        only_code(&parsed.root().body_text(0)?, true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_that_hold_a_code() {
        for (subject, code) in [
            ("Your verification code is 482913", "482913"),
            ("482913 is your Substack verification code", "482913"),
            ("482913 is your Instagram code", "482913"),
            ("Your Canva code is 482913", "482913"),
            ("Your Code - 48291", "48291"),
            ("Your login code: 7F3K-92QX", "7F3K-92QX"),
            ("Your Link verification code: 482 913", "482913"),
            ("Use 482-913 to sign in to Slack", "482913"),
            ("G-482913 is your Google verification code", "482913"),
            ("Mã Đăng Nhập: 482913", "482913"),
            ("482913 là mã xác nhận Facebook của bạn", "482913"),
            ("Mã OTP: 771204. Không chia sẻ mã này.", "771204"),
            ("ClickUp Verification Code: 4829", "4829"),
            ("OTP for your transaction: 482913", "482913"),
            ("Use OTP 482913 to confirm your order", "482913"),
            ("Tu código de verificación es 482913", "482913"),
            ("Seu código de verificação é 482913", "482913"),
            ("Votre code de vérification est 482913", "482913"),
            ("Votre code : 482913", "482913"),
            ("Ihr Bestätigungscode: 482913", "482913"),
            ("Ihr Code lautet 482913", "482913"),
            ("您的验证码是482913，请勿泄露", "482913"),
            ("認証コード：482913", "482913"),
            ("인증번호는 482913입니다", "482913"),
        ] {
            assert_eq!(in_subject(subject).as_deref(), Some(code), "{subject}");
        }
    }

    /// Each of these came up in real mail, or is the shape of something that did.
    #[test]
    fn numbers_that_are_something_else() {
        for subject in [
            "Use promo code SAVE2026 at checkout",
            "Zip code 90210 is outside our delivery area",
            "Your order code 48291377 has shipped",
            "Your access to the 2026 report ends soon",
            "Log in before 2026 to keep your 1500 points",
            "Invoice 483920: your login details have changed",
            "Meeting ID 842 913, passcode needed to join",
            "Gift card code 4829 1377 2210",
            "Call 0912 345 678 to verify your order",
            "Mã giảm giá SALE2026 cho đơn hàng của bạn",
            "Your code review for PR 4821 is ready",
            "Review the code in PR 4821",
            "Someone shared Henshin Code: 4829137's post",
            "[Binance] Verification Code - 2024-12-31 08:15:02 (UTC)",
            "Mã giao dịch/ Approval code : 482913",
            "Your flight confirmation code is AB1234",
            "Transaction code: 482913",
            "Your transaction ID is 482913",
            "Votre code promo 482913",
            "Su número de pedido es 482913",
            "Código promocional: SAVE2026",
            "Ihre Bestellung 482913 wurde versandt",
            "您的订单号482913已发货",
            "取件码 482913",
            "注文番号 482913",
            "クーポンコード: 482913",
            "주문번호 482913",
            "Su reserva: código de confirmación AB1234",
            "Re: Your verification code is 482913",
            "Fwd: Your verification code is 482913",
        ] {
            assert_eq!(in_subject(subject), None, "{subject}");
        }
    }

    #[test]
    fn two_codes_mean_neither() {
        assert_eq!(in_subject("Your code is 482913, your other code is 771204"), None);
        assert_eq!(in_subject("Your code is 482913. Again, your code is 482913.").as_deref(), Some("482913"));
    }

    fn mail(subject: &str, body: &str) -> Vec<u8> {
        format!("From: a@example.com\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n").into_bytes()
    }

    #[test]
    fn the_body_counts_only_when_the_subject_points_to_it() {
        let body = "Enter the following code: 482913\nThis code will expire in 10 minutes.\n";
        assert_eq!(in_message("Verify your email address", &mail("x", body)).as_deref(), Some("482913"));
        assert_eq!(in_message("Card payment successful", &mail("x", body)), None);
    }

    #[test]
    fn real_layouts_of_a_code_in_a_body() {
        for body in [
            // The code alone on a line, with the words about it before or after.
            "Please go to the page to confirm you own this email address: 482913 This code will expire in 15 minutes.",
            "Your code can only be used once. Never share this code with anyone.\n\n482 913\n\nDon't recognize this?",
            "Ihr Code lautet:\n\n482913\n\nDieser Code läuft bald ab.",
            "请输入以下验证码。\n482913\n此验证码10分钟内有效。",
            "and complete your sign in.\n\nVerification code:\n\n482913\n\n(This code will expire 10 minutes after it was sent.)\n\n\
             Amazon Web Services, Inc., 410 Terry Ave. North, Seattle, WA 98109-5210.",
        ] {
            assert_eq!(in_message("Verify your identity", &mail("x", body)).as_deref(), Some("482913"), "{body}");
        }
    }

    #[test]
    fn a_subject_can_point_to_a_body_with_no_code() {
        assert!(may_have_code("[GitHub] Please verify your device"));
        assert!(may_have_code("Mã xác thực giao dịch"));
        assert!(may_have_code("Vérifiez votre adresse e-mail"));
        assert!(may_have_code("验证码"));
        assert!(!may_have_code("Stop testing your code!"));
        assert!(!may_have_code("Your booking confirmation code"));
        assert_eq!(in_message("Verify your email", &mail("x", "Click the link below to verify your address.")), None);
    }

    #[test]
    fn a_message_that_cannot_be_read_has_no_code() {
        assert_eq!(in_message("Your verification code", b""), None);
    }
}
