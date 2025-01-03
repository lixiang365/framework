# 网络框架

一般是双线程，主线程执行逻辑
网络框架

单独IO线程 loop io事件

使用reactor 模型 基于mio

## 结构体

endpoint 保存端点状态，不管是server还是client

channel 维护发送缓冲区，其他一些接口的封装

接收到的消息通过队列发送到主线程，主线程做消息分发

