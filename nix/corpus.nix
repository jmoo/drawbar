# The private specimen corpus, pinned.
# ⚠️ Evaluating anything built from it needs read access to the repository.
builtins.fetchGit {
  rev = "c6f940562e51f2b4dcf2ba72ef4789ee45f27fd9";
  url = "git+ssh://git@github.com/jmoo/nord-corpus.git";
}
