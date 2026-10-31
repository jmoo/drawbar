# The models TLC checks Portable.tla against. Each is the baseline with its own
# constants, run by its suite's build, or by simulation where `suite` is null.
# A model checks one set of `checks`, or names the one property it `violates`
# and checks only that, which TLC must then report violated.
{
  baseline = {
    Carry = true;
    CertForks = true;
    Certify = true;
    CheckFirst = true;
    FoldedOnly = true;
    Instances = "{a, b}";
    MaxChaos = 0;
    MaxClones = 1;
    MaxDrops = 0;
    MaxEffects = 0;
    MaxEntries = 3;
    MaxFiles = 3;
    MaxLosses = 0;
    MaxRestores = 0;
    MaxWriters = 2;
    Numbered = true;
    Quarantine = true;
    ResumeMid = false;
    SealedOnly = true;
    UniqueNames = true;
  };

  checks = {
    liveness = {
      invariants = [ "AnchorsCertified" ];
      properties = [ "Convergence" ];
    };
    safety = {
      invariants = [
        "AnchorsCertified"
        "ChainOrder"
        "NothingIgnored"
        "ShownApart"
        "NoReapply"
        "NoSpuriousConflict"
        "UnforkedMerged"
        "NoMergedFork"
        "Agree"
        "NoOrphan"
        "DismissalsHold"
        "OwnDirectory"
        "Retained"
        "EffectAccounted"
        "DeliveredConverges"
        "DeliveredSettles"
      ];
      properties = [
        "Monotone"
        "NoFlip"
        "OwnFileNoRetire"
      ];
    };
  };

  models = {
    Anchors = {
      constants = {
        CertForks = false;
        MaxEntries = 4;
        MaxWriters = 1;
      };
      scenario = "Without forks found through certificates, once the original has moved two entries past a fork and compacted, a reader that still sees the original's deleted segment finds the fork and one that does not merges the original's branch.";
      suite = "toshokan-spec";
      violates = "Agree";
    };
    Branches = {
      checks = "safety";
      constants = {
        MaxEntries = 6;
        MaxFiles = 6;
      };
      scenario = "A clone, six entries and six files: both branches compacting past each other. Too large to explore whole.";
      suite = null;
    };
    Deep = {
      checks = "safety";
      constants = {
        MaxEffects = 1;
        MaxFiles = 4;
        MaxLosses = 1;
        MaxWriters = 3;
      };
      scenario = "A clone, effects and a lost local root together.";
      suite = "toshokan-spec-deep";
      symmetry = true;
    };
    Dismiss = {
      checks = "safety";
      constants = {
        MaxEntries = 4;
        MaxFiles = 4;
      };
      scenario = "Portable with four entries and four files: a clone and dismissals.";
      suite = "toshokan-spec-deep";
      symmetry = true;
    };
    Drop = {
      checks = "safety";
      constants = {
        MaxClones = 0;
        MaxDrops = 1;
      };
      scenario = "A sync client keeping an earlier version of a segment.";
      suite = "toshokan-spec";
    };
    Faults = {
      checks = "safety";
      constants = {
        MaxChaos = 1;
        MaxEffects = 1;
        MaxEntries = 2;
        MaxFiles = 4;
        MaxLosses = 1;
        MaxWriters = 3;
      };
      scenario = "Liveness with four files, every safety property.";
      suite = "toshokan-spec-deep";
      symmetry = true;
    };
    Folded = {
      constants = {
        FoldedOnly = false;
        MaxFiles = 5;
      };
      scenario = "A clone deletes a sealed segment of the original's that no snapshot folds.";
      suite = "toshokan-spec";
      violates = "Retained";
    };
    Liveness = {
      checks = "liveness";
      constants = {
        MaxChaos = 1;
        MaxEffects = 1;
        MaxEntries = 2;
        MaxLosses = 1;
        MaxWriters = 3;
      };
      scenario = "Convergence with a clone, a lost local root, an effect and one sync fault.";
      suite = "toshokan-spec";
    };
    LivenessDeep = {
      checks = "liveness";
      constants = {
        MaxChaos = 1;
        MaxEffects = 1;
        MaxEntries = 2;
        MaxFiles = 4;
        MaxLosses = 1;
        MaxWriters = 3;
      };
      scenario = "Convergence at the bounds of Faults.";
      suite = "toshokan-spec-deep";
    };
    LivenessRestore = {
      checks = "liveness";
      constants = {
        MaxClones = 0;
        MaxEntries = 2;
        MaxRestores = 1;
      };
      scenario = "Convergence with a restored folder.";
      suite = "toshokan-spec";
    };
    Names = {
      constants = {
        MaxFiles = 5;
        UniqueNames = false;
      };
      scenario = "Two histories name a segment alike, and a deletion by name reaches the other's.";
      suite = "toshokan-spec";
      violates = "Retained";
    };
    Portable = {
      checks = "safety";
      constants = { };
      scenario = "A clone, crashes, compaction.";
      suite = "toshokan-spec";
    };
    PortableDeep = {
      checks = "safety";
      constants.MaxFiles = 5;
      scenario = "Portable with five files.";
      suite = "toshokan-spec-deep";
      symmetry = true;
    };
    Recovery = {
      checks = "safety";
      constants = {
        MaxClones = 0;
        MaxEffects = 2;
        MaxLosses = 1;
      };
      scenario = "Two effects, a lost local root.";
      suite = "toshokan-spec";
    };
    Restore = {
      checks = "safety";
      constants = {
        MaxClones = 0;
        MaxEffects = 2;
        MaxRestores = 1;
      };
      scenario = "A restored folder, two effects.";
      suite = "toshokan-spec-deep";
    };
    RestoreDir = {
      checks = "safety";
      constants = {
        MaxClones = 0;
        MaxRestores = 1;
      };
      scenario = "A restored folder or directory.";
      suite = "toshokan-spec";
    };
    RestoreFork = {
      checks = "safety";
      constants = {
        MaxRestores = 1;
        MaxWriters = 3;
      };
      scenario = "A restored folder or directory, a clone.";
      suite = "toshokan-spec-deep";
      symmetry = true;
    };
    RestoreWide = {
      checks = "safety";
      constants = {
        MaxClones = 0;
        MaxEntries = 2;
        MaxFiles = 4;
        MaxRestores = 1;
      };
      scenario = "A restored folder or directory, four files.";
      suite = "toshokan-spec";
    };
    ResumeMid = {
      checks = "safety";
      constants = {
        MaxFiles = 4;
        ResumeMid = true;
      };
      scenario = "A clone resumed mid-segment, appending to its own copy of it.";
      suite = "toshokan-spec-deep";
      symmetry = true;
    };
    Resurface = {
      checks = "safety";
      constants = {
        MaxChaos = 1;
        MaxClones = 0;
        MaxFiles = 5;
        MaxWriters = 1;
      };
      scenario = "A deleted segment arriving after later compactions.";
      suite = "toshokan-spec";
    };
    Sealed = {
      constants = {
        MaxFiles = 5;
        SealedOnly = false;
      };
      scenario = "A clone deletes the segment the original still has open, and the original's next entry with it.";
      suite = "toshokan-spec";
      violates = "Retained";
    };
    Sync = {
      checks = "safety";
      constants = {
        MaxChaos = 2;
        MaxClones = 0;
        MaxEffects = 1;
        MaxFiles = 5;
      };
      scenario = "Two sync faults: torn, hidden or resurrected files.";
      suite = "toshokan-spec-deep";
    };
    Uncarried = {
      constants = {
        Carry = false;
        MaxClones = 0;
        MaxFiles = 5;
        MaxWriters = 1;
      };
      scenario = "A segment deleted two compactions ago arrives late, and without carried certificates its folded lines are shown as a run.";
      suite = "toshokan-spec";
      violates = "NoSpuriousConflict";
    };
    Uncertified = {
      constants = {
        Certify = false;
        MaxClones = 0;
        MaxEntries = 2;
        MaxWriters = 1;
      };
      scenario = "A snapshot that certifies no files leaves a segment below its anchor undeletable, and a reader shows its folded lines as a run.";
      suite = "toshokan-spec";
      violates = "NoSpuriousConflict";
    };
    Unchecked = {
      constants = {
        CheckFirst = false;
        MaxClones = 0;
        MaxFiles = 4;
        MaxRestores = 1;
        MaxWriters = 1;
      };
      scenario = "After a restore takes its head, a writer appends after it.";
      suite = "toshokan-spec";
      violates = "NoOrphan";
    };
    Unnumbered = {
      constants = {
        MaxClones = 0;
        MaxEntries = 2;
        MaxWriters = 1;
        Numbered = false;
      };
      scenario = "Without numbers, an entry whose predecessor has not arrived is shown as a run, then merged once it arrives.";
      suite = "toshokan-spec";
      violates = "NoFlip";
    };
    Unquarantined = {
      constants = {
        MaxEntries = 4;
        MaxFiles = 4;
        MaxRestores = 1;
        Quarantine = false;
      };
      scenario = "A reader that never saw a fork merges the branch a dismissal it holds names.";
      suite = "toshokan-spec";
      violates = "DismissalsHold";
    };
  };
}
