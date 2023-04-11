window.BENCHMARK_DATA = {
  "lastUpdate": 1681246988444,
  "repoUrl": "https://github.com/MystenLabs/sui",
  "entries": {
    "Benchmark": [
      {
        "commit": {
          "author": {
            "email": "sam@mystenlabs.com",
            "name": "Sam Blackshear",
            "username": "sblackshear"
          },
          "committer": {
            "email": "sam@mystenlabs.com",
            "name": "Sam Blackshear",
            "username": "sblackshear"
          },
          "distinct": true,
          "id": "f9da3c6c395935050eb4ce4ca49d075427003107",
          "message": "[rpc] use inner type for Balance in get_all_balances\n\nThis code used to say (e.g.) `SUI` for the type a each coin, but 3dd52052cc80449eb89d8901f27b9d366adb73cd incorrectly switched it to `Coin<SUI>`. Reverting to the original behavior",
          "timestamp": "2023-04-11T13:51:02-07:00",
          "tree_id": "f390e4652266d858b0116a7adf255c305ed194ba",
          "url": "https://github.com/MystenLabs/sui/commit/f9da3c6c395935050eb4ce4ca49d075427003107"
        },
        "date": 1681246986771,
        "tool": "cargo",
        "benches": [
          {
            "name": "persist_checkpoint",
            "value": 188861086,
            "range": "± 13540546",
            "unit": "ns/iter"
          },
          {
            "name": "get_checkpoint",
            "value": 450837,
            "range": "± 24478",
            "unit": "ns/iter"
          }
        ]
      }
    ]
  }
}