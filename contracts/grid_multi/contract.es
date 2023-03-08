{
  // decentralized multi-grid order contract for token/ERG trading, with ERG accumulation

  // script of order owner
  val ownerGroupElement = SELF.R4[GroupElement].get

  val selfIndex = CONTEXT.selfBoxIndex

  val validSwap = if(OUTPUTS.size < selfIndex ||
                     OUTPUTS(selfIndex).propositionBytes != SELF.propositionBytes) {
    false
  } else {
    val recreatedBox = OUTPUTS(selfIndex)

    // Collection of orders.
    // order is ((token_amount, state), (total_buy_price, total_sell_price))
    // state == TRUE means BUY, state == FALSE means SELL
    val currentOrders = SELF.R5[Coll[((Long, Boolean), (Long, Long))]].get
    val recreatedOrders = recreatedBox.R5[Coll[((Long, Boolean), (Long, Long))]].get

    // (true, (total change in tokens, total change in ERGs))
    // (false, _) means that at least one order is invalid
    val orderDiff = currentOrders.zip(recreatedOrders).fold((true, (0L, 0L)), {
      (acc: (Boolean, (Long, Long)),
      orders: (((Long, Boolean), (Long, Long)),
               ((Long, Boolean), (Long, Long)))) => {
        if(!acc._1) {
          acc
        } else {
          val currentOrder = orders._1
          val recreatedOrder = orders._2

          val currentAmount = currentOrder._1._1
          val recreatedAmount = recreatedOrder._1._1

          val currentState = currentOrder._1._2
          val recreatedState = recreatedOrder._1._2

          val currentBuyPrice = currentOrder._2._1
          val recreatedBuyPrice = recreatedOrder._2._1

          val currentSellPrice = currentOrder._2._2
          val recreatedSellPrice = recreatedOrder._2._2

          val tokensDiff = acc._2._1
          val ergsDiff = acc._2._2

          // Check that order parameters are not changed
          if (currentAmount != recreatedAmount
              || currentBuyPrice != recreatedBuyPrice
              || currentSellPrice != recreatedSellPrice) {
            (false, acc._2)
          } else if(currentState == recreatedState) {
            // No change in order
            acc
          } else if(currentState == true && recreatedState == false) {
            // Bought currentAmount of tokens, so we should have more tokens and less ERGs
            (true, (tokensDiff + currentAmount, ergsDiff - currentBuyPrice))
          } else if(currentState == false && recreatedState == true) {
            // Sold currentAmount of tokens, so we should have less tokens and more ERGs
            (true, (tokensDiff - currentAmount, ergsDiff + currentSellPrice))
          } else {
            // Should never happen
            (false, acc._2)
          }
        }
      }
    })

    val exchangeOk = if(orderDiff._1) {
      val diff = orderDiff._2
      val tokenDiff = diff._1
      val ergDiff = diff._2

      val valueDiff = recreatedBox.value - SELF.value
      val currentTokens = if(SELF.tokens.size > 0) SELF.tokens(0)._2 else 0L
      val recreatedTokens = if(recreatedBox.tokens.size > 0) recreatedBox.tokens(0)._2 else 0L

      val tokensDiff = recreatedTokens - currentTokens

      (
        tokensDiff != 0L &&
        valueDiff != 0L &&
        tokensDiff == tokenDiff &&
        valueDiff == ergDiff
      )
    } else {
      false
    }

    val tokenId = SELF.R6[Coll[Byte]].get

    val tokenIdOk = (
        recreatedBox.tokens.size <= 1 &&
        recreatedBox.tokens.forall{ (t: (Coll[Byte], Long)) => t._1 == tokenId}
    )

    // check conditions not related to trading here
    val orderRecreated = (
        recreatedBox.R4[GroupElement].get == SELF.R4[GroupElement].get &&
        // Order states are not checked here, because they are checked in orderDiff
        recreatedOrders.size == currentOrders.size &&
        recreatedBox.R6[Coll[Byte]].get == SELF.R6[Coll[Byte]].get
    )

    val metadataRecreated = if(SELF.R7[Coll[Byte]].isDefined) {
        // Enforce recreation of additional data which can be used for order grouping and tracking profits
        recreatedBox.R7[Coll[Byte]].get == SELF.R7[Coll[Byte]].get
    } else {
        true
    }

    val totalFee = OUTPUTS.fold(0L, {
      (fee:Long, b:Box) =>
        if (b.propositionBytes == FeeProposition) fee + b.value else fee
    })

    val feeOk = totalFee == MaxFee

    (
      tokenIdOk &&
      orderRecreated &&
      metadataRecreated &&
      exchangeOk &&
      feeOk
    )
  }

  sigmaProp(
    proveDlog(ownerGroupElement) || validSwap
  )
}
