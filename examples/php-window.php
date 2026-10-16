<?php
require_once __DIR__ . '/../php/Sweet.php';

use Sweet\Document;
use function Sweet\dominant;

$doc = new Document();
$d = dominant($doc);
$count = 0;
$doc->body()->setClassName('min-h-screen bg-zinc-100 p-8');
$label = $d->el('div', ['class' => 'text-2xl font-semibold'], $d->text(function() use (&$count) { return "Clicks: $count"; }));
$button = $d->el('button', ['class' => 'mt-4 rounded-md bg-blue-600 px-4 py-2 text-white', 'onClick' => function() use (&$count, $d) { $count++; $d->updateSync(); }], 'Click me');
$doc->body()->appendChild($label);
$doc->body()->appendChild($button);
$d->updateSync();

$window = $doc->window(['title' => 'Sweet PHP', 'width' => 720, 'height' => 480, 'dragAnywhere' => true]);
$window->run();
