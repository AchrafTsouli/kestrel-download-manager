//! The few texts Flux shows from outside its windows (tray menu, notifications), in the
//! interface language. The windows translate themselves (src/lib/i18n.ts).

/// The language to use for `setting` ("auto" follows Windows' display language).
pub fn resolve(setting: &str) -> &'static str {
    let code = if setting.is_empty() || setting == "auto" { windows_language() } else { setting.to_ascii_lowercase() };
    match code.split(['-', '_']).next().unwrap_or("") {
        "fr" => "fr",
        "ar" => "ar",
        _ => "en",
    }
}

#[cfg(windows)]
fn windows_language() -> String {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buffer = [0u16; 85];
    let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    if length > 1 { String::from_utf16_lossy(&buffer[..length as usize - 1]) } else { "en".into() }
}
#[cfg(not(windows))]
fn windows_language() -> String {
    std::env::var("LANG").unwrap_or_else(|_| "en".into())
}

const TABLE: &[(&str, &str, &str)] = &[
    ("Open Kestrel", "Ouvrir Kestrel", "فتح Kestrel"),
    ("Add URL…", "Ajouter un lien…", "إضافة رابط…"),
    ("Pause all", "Tout mettre en pause", "إيقاف الكل مؤقتًا"),
    ("Resume all", "Tout reprendre", "استئناف الكل"),
    ("Exit", "Quitter", "خروج"),
    ("Add new download", "Ajouter un téléchargement", "إضافة تنزيل جديد"),
    ("Add batch download", "Ajouter un téléchargement groupé", "إضافة تنزيل مجمّع"),
    ("Downloads in progress", "Téléchargements en cours", "التنزيلات الجارية"),
    ("Speed Limiter", "Limiteur de vitesse", "محدد السرعة"),
    ("Turn on", "Activer", "تشغيل"),
    ("Turn off", "Désactiver", "إيقاف"),
    ("Scheduler", "Planificateur", "المجدول"),
    ("Start main download queue", "Démarrer la file principale", "بدء قائمة التنزيل الرئيسية"),
    ("Stop main download queue", "Arrêter la file principale", "إيقاف قائمة التنزيل الرئيسية"),
    ("Options", "Options", "الخيارات"),
    ("Download complete", "Téléchargement terminé", "اكتمل التنزيل"),
    ("Download failed", "Échec du téléchargement", "فشل التنزيل"),
];

pub fn tr(language: &str, english: &'static str) -> &'static str {
    TABLE
        .iter()
        .find(|(en, _, _)| *en == english)
        .map(|(en, fr, ar)| match language {
            "fr" => *fr,
            "ar" => *ar,
            _ => *en,
        })
        .unwrap_or(english)
}

#[cfg(test)]
mod tests {
    #[test]
    fn texts_follow_the_language() {
        assert_eq!(super::tr("fr", "Exit"), "Quitter");
        assert_eq!(super::tr("en", "Exit"), "Exit");
        assert_eq!(super::tr("ar", "Not in the table"), "Not in the table");
        assert_eq!(super::resolve("fr-CA"), "fr");
        assert_eq!(super::resolve("de"), "en");
        assert!(["en", "fr", "ar"].contains(&super::resolve("auto")));
    }
}
