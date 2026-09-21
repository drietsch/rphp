<?php
// A worker serves many requests: nothing of the previous request's session
// (its data, its id, its `active` status) may reach this one.
$before = session_status() === PHP_SESSION_ACTIVE ? 'active' : 'none';
session_start();
echo "before=$before had=", isset($_SESSION['n']) ? 'yes' : 'no', " n=", $_SESSION['n'] = ($_SESSION['n'] ?? 0) + 1, "\n";
echo "tz=", date_default_timezone_get(), " mb=", mb_internal_encoding(), " strtok=", var_export(strtok(' '), true), "\n";
date_default_timezone_set('Europe/Berlin');
mb_internal_encoding('ISO-8859-1');
strtok('a b c', ' ');
