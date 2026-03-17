path = r'C:\Users\Baxter\Desktop\Iris\crates\iris-ui\bootstrap.rs'
s = open(path, 'r', encoding='utf-8').read()
bal = 0
for i,ch in enumerate(s):
    if ch == '{':
        bal += 1
    elif ch == '}':
        bal -= 1
    if bal < 0:
        print('Negative balance at index', i)
        break
print('final balance', bal)
# print around impl IrisRuntime start and the position 554
lines = s.splitlines()
print('\n--- around impl IrisRuntime ---')
for ln in range(160, 190):
    print(ln+1, lines[ln])
print('\n--- around 540-560 ---')
for ln in range(540, 560):
    print(ln+1, lines[ln])
