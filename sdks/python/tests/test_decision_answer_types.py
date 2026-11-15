"""A decision's typed answer to a question set keeps its typed entries."""

import sys
import typing

from meerkat.generated import types as generated

# Python 3.10's typing lacks Required, and its get_type_hints keeps the
# typing_extensions qualifiers that 3.11+ strips.
if sys.version_info >= (3, 11):
    from typing import Required, get_type_hints
else:
    from typing_extensions import Required, get_type_hints


def test_an_ask_user_answer_lists_the_named_question_answer() -> None:
    hints = get_type_hints(generated.WireDecisionAnswerAskUser, include_extras=True)
    assert hints["kind"] == Required[typing.Literal["ask_user"]]
    assert hints["answers"] == Required[list[generated.WireAskUserQuestionAnswer]]

    # The entry names its question and carries the typed answer.
    entry = get_type_hints(generated.WireAskUserQuestionAnswer)
    assert entry == {"answer": generated.WireAskUserAnswer, "question_id": str}
    arms = typing.get_args(generated.WireAskUserAnswer)
    assert set(arms) == {
        generated.WireAskUserAnswerChoice,
        generated.WireAskUserAnswerFreeText,
        generated.WireAskUserAnswerUnanswered,
        generated.WireAskUserAnswerUnknown,
    }
