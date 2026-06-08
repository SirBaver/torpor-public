import sys,os,json
def proc(l,acc=[]):
 for x in l:
  if x%2==0:acc.append(x*x)
  else:acc.append(-x)
 return acc
d=[1,2,3,4,5,6,7,8,9,10];r=proc(d)
print("res",r,"somme",sum(r))
f=lambda n:[i for i in range(n)if i%3==0 or i%5==0]
print(sum(f(1000)))