"""Fictional, dependency-free organization contribution for Forge."""


def organization():
    return {
        "schema_version": 1,
        "id": "workplace",
        "version": "1.0.0",
        "label": "Example workplace",
        "description": "Make room for the work that matters. Start with a recurring task or an unanswered question.",
        "guide_context": (
            "Explore useful workplace outcomes. Ask whether this is for one person, "
            "a team or an organization. Learn about the user's existing sources, "
            "approval boundaries and scale before suggesting tools or agent classes."
        ),
        "starting_examples": [
            {
                "id": "delivery",
                "title": "See what needs attention",
                "outcome": "A clear view of commitments, changes and work that is stuck.",
                "initial_brief": (
                    "Help me understand what needs attention across our team's work. "
                    "I'd like a useful summary of progress, blockers and decisions. "
                    "Ask who it is for and where the relevant context lives."
                ),
                "context": "Useful for teams with scattered updates and shared commitments.",
                "integration_hints": ["Issue tracking", "Team conversations", "Project documents"],
            },
            {
                "id": "research",
                "title": "Get an answer with evidence",
                "outcome": "Turn scattered knowledge into an answer you can check.",
                "initial_brief": (
                    "I want help investigating workplace questions using our existing "
                    "knowledge. Show the evidence and unresolved disagreements. "
                    "Help me choose a first question before designing the system."
                ),
                "integration_hints": ["Enterprise search", "Documents"],
            },
            {
                "id": "requests",
                "title": "Keep requests moving",
                "outcome": "Know what was asked, who owns it and what happens next.",
                "initial_brief": (
                    "Help our team keep track of incoming requests and follow-ups. "
                    "Ask how requests arrive, who handles them and what needs a "
                    "person's approval before proposing any automation."
                ),
                "integration_hints": ["Team conversations", "Issue tracking"],
            },
        ],
    }
