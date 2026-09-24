<?php
// MessageFormatter's date and time styles, date skeletons, number
// skeletons, and the rule-based spellout / ordinal / duration formats.
$ts = 86400 * 365 + 3723;
foreach (['en', 'de', 'fr', 'ja'] as $l) {
    echo $l, ': ', MessageFormatter::formatMessage($l, '{0,date} | {0,time} | {0,date,short} | {0,time,short} | {0,date,long} | {0,date,full}', [$ts]), "\n";
    echo $l, ': ', MessageFormatter::formatMessage($l, '{0,date,yyyy-MM-dd HH:mm} | {0,date,::yMMMd} | {0,date,::jmm} | {0,date,::yMMMMEEEEd}', [$ts]), "\n";
    echo $l, ': ', MessageFormatter::formatMessage($l, '{0,number,::percent} | {0,number,::currency/EUR} | {0,number,::.00} | {0,number,::group-off} | {0,number,::sign-always}', [1234.5]), "\n";
    echo $l, ': ', MessageFormatter::formatMessage($l, '{0,spellout} | {0,ordinal} | {0,duration}', [4242]), "\n";
}
$d = new DateTime('2020-01-02 03:04:05', new DateTimeZone('UTC'));
echo MessageFormatter::formatMessage('en', '{0,date,medium}', [$d]), "\n";
foreach ([0, 1, 2, 11, 21, 42, 100, 101, 1999, 1000000, -7, 1.5, 0.25] as $n) {
    echo $n, ': ', MessageFormatter::formatMessage('en', '{0,spellout} | {0,spellout,%spellout-ordinal} | {0,spellout,%spellout-cardinal-verbose} | {0,ordinal}', [$n]), "\n";
}
foreach (['de', 'fr', 'es', 'ru', 'it', 'nl'] as $l) {
    echo $l, ': ', MessageFormatter::formatMessage($l, '{0,spellout} | {1,spellout} | {2,spellout}', [21, 1234, 1.5]), "\n";
}
