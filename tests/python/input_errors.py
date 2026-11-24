"""绑定层参数错误的公开字段契约。"""


def check_input_error(error, field, write=None):
    assert error.schema_version == 1
    assert error.code == "TDB_INVALID_INPUT"
    assert error.target == {"field": field}
    assert error.stage is None
    assert error.write == write
    assert list(error.details) == ["reason"]
    assert isinstance(error.details["reason"], str) and error.details["reason"]
