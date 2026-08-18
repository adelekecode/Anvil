/// First visible character of a name, for avatars.
///
/// Written by hand rather than reaching for `String.characters` so that an
/// empty name, a name starting with an emoji, or a name that is pure whitespace
/// all produce something rather than throwing. Names come off the air from
/// strangers; none of those inputs are hypothetical.
String initialOf(String name) {
  final trimmed = name.trim();
  if (trimmed.isEmpty) return '?';

  // Take a whole rune, so a multi-byte first character is not sliced in half.
  final first = trimmed.runes.first;
  return String.fromCharCode(first).toUpperCase();
}
