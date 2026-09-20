<?php session_start(); $_SESSION['n'] = ($_SESSION['n'] ?? 0) + 1; echo "n=", $_SESSION['n'], " id-len=", strlen(session_id()), "\n";
