<?php
// IntlBreakIterator: word / line / sentence / character / title
// boundaries with rule statuses, over several scripts and locales.
$texts = [
    "Hello, world! How are you? I'm fine.",
    "The quick (\"brown\") fox can't jump 32.3 feet, right?",
    "Dr. Müller wohnt in der Hauptstraße 5. Er sagt: „Guten Tag!“ Danach geht er.",
    "Съешь же ещё этих мягких французских булок. Конец!",
    "Η γρήγορη καφέ αλεπού; Ναι. Τέλος.",
    "مرحبا بالعالم. كيف حالك؟",
    "我们今天去北京大学学习中文。你呢？",
    "ภาษาไทยเป็นภาษาที่สวยงาม ฉันชอบเรียน",
    "e\u{301}👨‍👩‍👧 🇩🇪x 👍🏽",
    "user@example.com https://example.com/a?b=1 192.168.0.1 \$1,234.56 50%",
    "a\r\nb\nc\u{2029}d\u{85}e",
    "", "  ",
    "CAPS lower MiXeD iPhone O'Neil McDonald's",
];
foreach (['createWordInstance', 'createLineInstance', 'createSentenceInstance', 'createCharacterInstance', 'createTitleInstance'] as $m) {
    foreach (['en_US', 'de', 'el', 'th', 'de@lb=loose'] as $loc) {
        $b = IntlBreakIterator::$m($loc);
        echo $m, ' ', $loc, ' ', get_class($b), ' ', $b->getLocale(Locale::VALID_LOCALE), '|', $b->getLocale(Locale::ACTUAL_LOCALE), "\n";
        foreach ($texts as $i => $t) {
            $b->setText($t);
            $out = [];
            foreach ($b as $k => $v) {
                $out[] = "$k:$v:" . $b->getRuleStatus();
            }
            echo "  #$i ", implode(' ', $out), "\n";
        }
    }
}
$w = IntlBreakIterator::createWordInstance('en');
$w->setText('Hello big world');
var_dump($w->getRuleStatusVec());
$w->next();
var_dump($w->getRuleStatusVec(), $w->getText(), strlen($w->getRules()) > 100);
var_dump(get_class($w->getIterator()));
